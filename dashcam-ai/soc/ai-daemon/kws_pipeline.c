#include "kws_pipeline.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <math.h>
#include <stdatomic.h>
#include "rknn_api.h"
/* TODO: #include <alsa/asoundlib.h> */

/* ── Internal state ────────────────────────────────────────────────────── */

static rknn_context    g_ctx          = 0;
static atomic_int      g_running      = 0;
static int             g_consec_hits  = 0;   /* consecutive positive frames */

/* Sliding MFCC feature buffer [KWS_CONTEXT_FRAMES][KWS_N_MFCC] */
static float g_mfcc_buf[KWS_CONTEXT_FRAMES][KWS_N_MFCC];
static int   g_buf_idx = 0;   /* next write position (ring) */
static int   g_buf_fill = 0;  /* frames filled so far (0..KWS_CONTEXT_FRAMES) */

/* ── MFCC feature extraction ───────────────────────────────────────────── */

/* Pre-emphasis filter coefficient */
#define PRE_EMPHASIS  0.97f

static void extract_mfcc(const int16_t *pcm, int n_samples, float *mfcc_out) {
    /* Step 1: pre-emphasis */
    float emphasized[KWS_FRAME_SAMPLES];
    emphasized[0] = (float)pcm[0];
    for (int i = 1; i < n_samples; i++)
        emphasized[i] = (float)pcm[i] - PRE_EMPHASIS * (float)pcm[i - 1];

    /* Step 2: apply Hamming window */
    for (int i = 0; i < n_samples; i++) {
        float w = 0.54f - 0.46f * cosf(2.0f * M_PI * i / (n_samples - 1));
        emphasized[i] *= w;
    }

    /* Step 3: FFT → power spectrum
     * TODO: use kiss_fft or pffft for a real implementation */
    float power[KWS_FRAME_SAMPLES / 2 + 1];
    memset(power, 0, sizeof(power));
    /* power[k] = |FFT(emphasized)[k]|^2 */

    /* Step 4: apply mel filterbank (KWS_N_MFCC triangular filters) */
    float mel_energies[KWS_N_MFCC];
    memset(mel_energies, 0, sizeof(mel_energies));
    /* TODO: apply precomputed filterbank weights to power spectrum */

    /* Step 5: log + DCT → MFCC coefficients */
    for (int m = 0; m < KWS_N_MFCC; m++) {
        float log_e = logf(fmaxf(mel_energies[m], 1e-10f));
        float coeff = 0.0f;
        /* DCT-II: c[m] = sum_k log_e[k] * cos(pi*m*(k+0.5)/N) */
        /* TODO: implement DCT */
        mfcc_out[m] = coeff + log_e;   /* placeholder */
    }
}

/* Flatten the sliding MFCC ring buffer into a contiguous input tensor
 * in chronological order (oldest frame first) */
static void flatten_mfcc_buf(float *out) {
    int start = (g_buf_fill < KWS_CONTEXT_FRAMES)
                    ? 0
                    : (g_buf_idx % KWS_CONTEXT_FRAMES);
    int frames = (g_buf_fill < KWS_CONTEXT_FRAMES)
                     ? g_buf_fill
                     : KWS_CONTEXT_FRAMES;
    for (int i = 0; i < frames; i++) {
        int src = (start + i) % KWS_CONTEXT_FRAMES;
        memcpy(out + i * KWS_N_MFCC, g_mfcc_buf[src], KWS_N_MFCC * sizeof(float));
    }
    /* Zero-pad if buffer not yet full */
    for (int i = frames; i < KWS_CONTEXT_FRAMES; i++)
        memset(out + i * KWS_N_MFCC, 0, KWS_N_MFCC * sizeof(float));
}

/* ── Public API ────────────────────────────────────────────────────────── */

int kws_pipeline_init(const char *model_path) {
    int ret = rknn_init(&g_ctx, (void *)model_path, 0, 0, NULL);
    if (ret < 0) {
        fprintf(stderr, "[kws] rknn_init failed: %d\n", ret);
        return -1;
    }
    memset(g_mfcc_buf, 0, sizeof(g_mfcc_buf));
    g_buf_idx = 0; g_buf_fill = 0; g_consec_hits = 0;
    fprintf(stderr, "[kws] loaded %s\n", model_path);
    return 0;
}

int kws_pipeline_process_frame(const int16_t *pcm_frame, kws_result_t *result) {
    /* Extract MFCC features for this frame */
    float mfcc[KWS_N_MFCC];
    extract_mfcc(pcm_frame, KWS_FRAME_SAMPLES, mfcc);

    /* Push into sliding buffer */
    memcpy(g_mfcc_buf[g_buf_idx % KWS_CONTEXT_FRAMES], mfcc, sizeof(mfcc));
    g_buf_idx++;
    if (g_buf_fill < KWS_CONTEXT_FRAMES) g_buf_fill++;

    /* Need at least KWS_CONTEXT_FRAMES frames before running inference */
    if (g_buf_fill < KWS_CONTEXT_FRAMES) return 0;

    /* Flatten ring buffer into contiguous input tensor */
    float input_tensor[KWS_CONTEXT_FRAMES * KWS_N_MFCC];
    flatten_mfcc_buf(input_tensor);

    /* Run RKNN inference */
    rknn_input inputs[1] = {{
        .index        = 0,
        .buf          = input_tensor,
        .size         = sizeof(input_tensor),
        .pass_through = 0,
        .type         = RKNN_TENSOR_FLOAT32,
        .fmt          = RKNN_TENSOR_UNDEFINED,
    }};
    rknn_inputs_set(g_ctx, 1, inputs);
    if (rknn_run(g_ctx, NULL) < 0) return -1;

    /* Output: [wake_prob, non_wake_prob] */
    rknn_output outputs[1] = {{ .want_float = 1 }};
    rknn_outputs_get(g_ctx, 1, outputs, NULL);
    float wake_prob = ((float *)outputs[0].buf)[0];
    rknn_outputs_release(g_ctx, 1, outputs);

    /* Consecutive hit counter — debounce against spurious detections */
    if (wake_prob >= KWS_CONF_THRESHOLD) {
        g_consec_hits++;
    } else {
        g_consec_hits = 0;
    }

    if (g_consec_hits >= KWS_TRIGGER_FRAMES) {
        g_consec_hits = 0;   /* reset to avoid repeated firing */
        result->confidence   = wake_prob;
        result->timestamp_us = /* TODO: monotonic clock */ 0;
        return 1;   /* wake event */
    }
    return 0;
}

int kws_pipeline_run_loop(const char *alsa_device,
                          kws_wake_cb_t on_wake, void *ctx) {
    atomic_store(&g_running, 1);

    /* TODO: open ALSA capture device
     * snd_pcm_t *pcm;
     * snd_pcm_open(&pcm, alsa_device, SND_PCM_STREAM_CAPTURE, 0);
     * snd_pcm_set_params(pcm, SND_PCM_FORMAT_S16_LE, SND_PCM_ACCESS_RW_INTERLEAVED,
     *                    1, KWS_SAMPLE_RATE, 1, KWS_FRAME_MS * 1000);
     */

    int16_t frame_buf[KWS_FRAME_SAMPLES];
    kws_result_t result;

    while (atomic_load(&g_running)) {
        /* TODO: snd_pcm_readi(pcm, frame_buf, KWS_FRAME_SAMPLES); */
        memset(frame_buf, 0, sizeof(frame_buf));   /* placeholder */

        int detected = kws_pipeline_process_frame(frame_buf, &result);
        if (detected == 1 && on_wake) {
            on_wake(&result, ctx);
        } else if (detected < 0) {
            fprintf(stderr, "[kws] inference error\n");
        }
    }

    /* TODO: snd_pcm_close(pcm); */
    return 0;
}

void kws_pipeline_stop(void) {
    atomic_store(&g_running, 0);
}

void kws_pipeline_destroy(void) {
    kws_pipeline_stop();
    if (g_ctx) { rknn_destroy(g_ctx); g_ctx = 0; }
}

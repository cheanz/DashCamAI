#include "stt_pipeline.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <math.h>
#include "rknn_api.h"

/* ── Whisper architecture constants ────────────────────────────────────── */

#define MEL_BINS        80       /* log-mel spectrogram frequency bins */
#define MEL_FRAMES      3000     /* 30s * 100 frames/s */
#define FRAME_SIZE      400      /* 25ms window at 16kHz */
#define HOP_SIZE        160      /* 10ms hop at 16kHz */

/* Whisper token IDs (GPT-2 tokenizer) */
#define TOKEN_SOT       50258    /* <|startoftranscript|> */
#define TOKEN_TRANSCRIBE 50359
#define TOKEN_NO_TIMESTAMPS 50363
#define TOKEN_EOT       50257    /* <|endoftext|> */

/* ── Internal state ────────────────────────────────────────────────────── */

static rknn_context g_encoder_ctx = 0;
static rknn_context g_decoder_ctx = 0;

/* Mel filterbank (precomputed at init) */
static float g_mel_filters[MEL_BINS][FRAME_SIZE / 2 + 1];

/* ── Log-mel spectrogram ───────────────────────────────────────────────── */

static void hann_window(float *window, int n) {
    for (int i = 0; i < n; i++)
        window[i] = 0.5f * (1.0f - cosf(2.0f * M_PI * i / (n - 1)));
}

static void compute_log_mel(const int16_t *pcm, int n_samples,
                             float *mel_out /* [MEL_BINS][MEL_FRAMES] */) {
    float window[FRAME_SIZE];
    hann_window(window, FRAME_SIZE);

    memset(mel_out, 0, sizeof(float) * MEL_BINS * MEL_FRAMES);

    int n_frames = (n_samples - FRAME_SIZE) / HOP_SIZE + 1;
    if (n_frames > MEL_FRAMES) n_frames = MEL_FRAMES;

    for (int f = 0; f < n_frames; f++) {
        float fft_in[FRAME_SIZE];
        const int16_t *frame = pcm + f * HOP_SIZE;

        /* Apply Hann window and normalize */
        for (int i = 0; i < FRAME_SIZE; i++)
            fft_in[i] = (frame[i] / 32768.0f) * window[i];

        /* TODO: compute real FFT (use kiss_fft or pffft for efficiency) */
        /* float fft_out[FRAME_SIZE / 2 + 1];  power spectrum */

        /* Apply mel filterbank */
        /* for (int m = 0; m < MEL_BINS; m++) {
         *     float energy = 0;
         *     for (int k = 0; k <= FRAME_SIZE / 2; k++)
         *         energy += g_mel_filters[m][k] * fft_out[k];
         *     mel_out[m * MEL_FRAMES + f] = log10f(fmaxf(energy, 1e-10f));
         * } */
    }
}

/* ── Token decoding ────────────────────────────────────────────────────── */

/* Whisper language token range: 50259..50357 maps to ISO 639-1 codes.
 * This table covers the most common languages for this product. */
static const struct { int token; const char *lang; } LANG_TOKENS[] = {
    { 50260, "en" }, { 50261, "zh" }, { 50262, "de" }, { 50263, "es" },
    { 50264, "ru" }, { 50265, "ko" }, { 50266, "fr" }, { 50267, "ja" },
    { 50268, "pt" }, { 50269, "tr" }, { 50270, "pl" }, { 50271, "ca" },
    { 50272, "nl" }, { 50273, "ar" }, { 50274, "sv" }, { 50275, "it" },
};
#define N_LANG_TOKENS (sizeof(LANG_TOKENS) / sizeof(LANG_TOKENS[0]))

static const char *token_to_lang(int token_id) {
    for (size_t i = 0; i < N_LANG_TOKENS; i++)
        if (LANG_TOKENS[i].token == token_id) return LANG_TOKENS[i].lang;
    return "unk";
}

/* ── Public API ────────────────────────────────────────────────────────── */

int stt_pipeline_init(const char *model_path) {
    /* Whisper tiny splits into encoder + decoder; both compiled as one .rknn
     * or as separate graphs depending on RKNN Toolkit export.
     * Attempt single-graph load first; fall back to split if needed. */
    int ret = rknn_init(&g_encoder_ctx, (void *)model_path, 0, 0, NULL);
    if (ret < 0) {
        fprintf(stderr, "[stt] rknn_init failed: %d\n", ret);
        return -1;
    }

    /* TODO: precompute mel filterbank coefficients (triangular filters)
     * using HTK formula:
     *   f_mel = 2595 * log10(1 + f_hz / 700)
     * and store in g_mel_filters[MEL_BINS][FRAME_SIZE/2+1] */

    fprintf(stderr, "[stt] loaded %s\n", model_path);
    return 0;
}

int stt_pipeline_run(const int16_t *pcm_samples, int n_samples,
                     stt_result_t *result) {
    if (!g_encoder_ctx || !pcm_samples || !result) return -1;

    /* Step 1: compute log-mel spectrogram */
    float *mel = calloc(MEL_BINS * MEL_FRAMES, sizeof(float));
    if (!mel) return -1;
    compute_log_mel(pcm_samples, n_samples, mel);

    /* Step 2: run encoder */
    rknn_input enc_in[1] = {{
        .index        = 0,
        .buf          = mel,
        .size         = MEL_BINS * MEL_FRAMES * sizeof(float),
        .pass_through = 0,
        .type         = RKNN_TENSOR_FLOAT32,
        .fmt          = RKNN_TENSOR_UNDEFINED,
    }};
    rknn_inputs_set(g_encoder_ctx, 1, enc_in);
    int ret = rknn_run(g_encoder_ctx, NULL);
    free(mel);
    if (ret < 0) return -1;

    /* Get encoder hidden states */
    rknn_output enc_out[1] = {{ .want_float = 1 }};
    rknn_outputs_get(g_encoder_ctx, 1, enc_out, NULL);

    /* Step 3: autoregressive decoder — greedy decoding
     * Seed with [TOKEN_SOT, language_token, TOKEN_TRANSCRIBE, TOKEN_NO_TIMESTAMPS]
     * then sample until TOKEN_EOT or max_tokens.
     *
     * TODO: implement decoder loop using g_decoder_ctx:
     *   int tokens[448] = { TOKEN_SOT, ... };
     *   for (int t = 4; t < 448; t++) {
     *       run decoder with encoder states + tokens[0..t-1]
     *       next_token = argmax(logits)
     *       if (next_token == TOKEN_EOT) break;
     *       tokens[t] = next_token;
     *   }
     *   decode token ids → UTF-8 string using GPT-2 tokenizer
     *   extract language token → result->lang
     */

    /* Placeholder output */
    snprintf(result->transcript, STT_MAX_TRANSCRIPT, "[stt decode not yet implemented]");
    snprintf(result->lang, sizeof(result->lang), "en");
    result->confidence  = 0.0f;
    result->timestamp_us = 0;

    rknn_outputs_release(g_encoder_ctx, 1, enc_out);
    return 0;
}

void stt_pipeline_destroy(void) {
    if (g_encoder_ctx) { rknn_destroy(g_encoder_ctx); g_encoder_ctx = 0; }
    if (g_decoder_ctx) { rknn_destroy(g_decoder_ctx); g_decoder_ctx = 0; }
}

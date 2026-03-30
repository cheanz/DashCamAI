#pragma once
#include <stdint.h>

/* Audio spec for KWS */
#define KWS_SAMPLE_RATE     16000    /* Hz */
#define KWS_FRAME_MS        30       /* feature extraction window */
#define KWS_FRAME_SAMPLES   (KWS_SAMPLE_RATE * KWS_FRAME_MS / 1000)   /* 480 samples */
#define KWS_N_MFCC          40       /* MFCC feature dimensions */
#define KWS_CONTEXT_FRAMES  49       /* 1.49s of context (~DNN-HMM window) */

/* Minimum consecutive positive frames before firing wake event */
#define KWS_TRIGGER_FRAMES  3

/* Confidence threshold for a positive detection */
#define KWS_CONF_THRESHOLD  0.85f

typedef struct {
    float    confidence;
    uint64_t timestamp_us;
} kws_result_t;

/**
 * kws_pipeline_init — load KWS driving-mode model into RKNN context.
 * @param model_path  path to kws-driving.rknn
 * @return 0 on success, -1 on failure
 */
int  kws_pipeline_init(const char *model_path);

/**
 * kws_pipeline_process_frame — process one audio frame (KWS_FRAME_SAMPLES).
 *
 * Maintains an internal MFCC feature buffer across calls (sliding window).
 * Returns 1 and fills result when wake-word confidence exceeds threshold
 * for KWS_TRIGGER_FRAMES consecutive frames.
 * Returns 0 when no wake event, -1 on error.
 *
 * Designed to be called in a tight loop from kws_pipeline_run_loop().
 *
 * @param pcm_frame   KWS_FRAME_SAMPLES of mono 16kHz 16-bit PCM
 * @param result      populated only when return value == 1
 */
int  kws_pipeline_process_frame(const int16_t *pcm_frame, kws_result_t *result);

/**
 * kws_pipeline_run_loop — blocking inference loop.
 *
 * Opens the ALSA device, reads frames, and calls kws_pipeline_process_frame().
 * Calls on_wake_detected(result, ctx) when a wake event fires.
 * Returns only on error or when kws_pipeline_stop() is called.
 *
 * @param alsa_device   e.g. "hw:0,0"
 * @param on_wake       callback invoked on detection
 * @param ctx           user context passed to callback
 */
typedef void (*kws_wake_cb_t)(const kws_result_t *result, void *ctx);
int  kws_pipeline_run_loop(const char *alsa_device,
                           kws_wake_cb_t on_wake, void *ctx);

void kws_pipeline_stop(void);
void kws_pipeline_destroy(void);

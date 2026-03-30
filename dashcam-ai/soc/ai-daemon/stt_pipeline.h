#pragma once
#include <stdint.h>
#include "../shared/events.h"

/* Audio input spec expected by Whisper tiny */
#define STT_SAMPLE_RATE     16000    /* Hz */
#define STT_CHUNK_DURATION  30       /* seconds — Whisper context window */
#define STT_CHUNK_SAMPLES   (STT_SAMPLE_RATE * STT_CHUNK_DURATION)

/* Maximum supported transcript length */
#define STT_MAX_TRANSCRIPT  512

/* ISO 639-1 language codes populated by Whisper's language detection */
#define STT_LANG_AUTO       "auto"   /* let Whisper detect */

typedef struct {
    char     transcript[STT_MAX_TRANSCRIPT];
    char     lang[8];                /* detected language, e.g. "zh", "en", "es" */
    float    confidence;
    uint64_t timestamp_us;
} stt_result_t;

/**
 * stt_pipeline_init — load Whisper tiny model into RKNN context.
 * @param model_path  path to whisper-tiny.rknn
 * @return 0 on success, -1 on failure
 */
int  stt_pipeline_init(const char *model_path);

/**
 * stt_pipeline_run — run STT inference on a PCM audio buffer.
 *
 * Expects mono, 16kHz, 16-bit signed PCM.
 * Internally computes log-mel spectrogram before passing to the encoder.
 * Language is auto-detected by Whisper's language token.
 *
 * @param pcm_samples  pointer to raw PCM data
 * @param n_samples    number of samples (max STT_CHUNK_SAMPLES)
 * @param result       output transcript + language + confidence
 * @return 0 on success, -1 on failure
 */
int  stt_pipeline_run(const int16_t *pcm_samples, int n_samples,
                      stt_result_t *result);

void stt_pipeline_destroy(void);

#pragma once
#include <stdint.h>
#include "../shared/events.h"

/* Maximum input tokens for the intent classifier */
#define INTENT_MAX_INPUT_LEN   128

/* Confidence below which the classifier falls back to INTENT_COMPLEX_DIALOGUE */
#define INTENT_CONF_THRESHOLD  0.70f

/* Simple command labels the edge can handle offline */
typedef enum {
    SIMPLE_CMD_NAVIGATION  = 0,
    SIMPLE_CMD_MEDIA       = 1,
    SIMPLE_CMD_VOLUME      = 2,
    SIMPLE_CMD_EMERGENCY   = 3,
    SIMPLE_CMD_CAMERA      = 4,
} simple_cmd_t;

typedef struct {
    intent_type_t intent;        /* INTENT_SIMPLE_COMMAND / COMPLEX_DIALOGUE / TRANSLATION */
    simple_cmd_t  simple_cmd;    /* valid only when intent == INTENT_SIMPLE_COMMAND */
    float         confidence;
    char          detected_lang[8];
} intent_result_t;

/**
 * intent_pipeline_init — load ONNX intent classifier via ONNX Runtime.
 * @param model_path  path to intent-classifier.onnx
 * @return 0 on success, -1 on failure
 */
int  intent_pipeline_init(const char *model_path);

/**
 * intent_pipeline_run — classify intent from a transcript.
 *
 * Tokenizes the transcript, runs the ONNX classifier, and returns the
 * most likely intent class and confidence.
 *
 * If confidence < INTENT_CONF_THRESHOLD, defaults to INTENT_COMPLEX_DIALOGUE
 * to route to the cloud rather than risk a wrong edge response.
 *
 * @param transcript   UTF-8 text from STT
 * @param lang         ISO 639-1 language code (from STT result)
 * @param result       output intent classification
 * @return 0 on success, -1 on failure
 */
int  intent_pipeline_run(const char *transcript, const char *lang,
                         intent_result_t *result);

void intent_pipeline_destroy(void);

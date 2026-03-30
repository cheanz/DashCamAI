#include "intent_pipeline.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <math.h>

/* ONNX Runtime C API */
#include "onnxruntime_c_api.h"

/* ── Internal state ────────────────────────────────────────────────────── */

static const OrtApi       *g_ort     = NULL;
static OrtEnv             *g_env     = NULL;
static OrtSession         *g_session = NULL;
static OrtSessionOptions  *g_opts    = NULL;
static OrtMemoryInfo      *g_mem     = NULL;

/* Label map — must match training label order */
static const struct { intent_type_t intent; simple_cmd_t cmd; const char *label; }
INTENT_LABELS[] = {
    { INTENT_SIMPLE_COMMAND,   SIMPLE_CMD_NAVIGATION, "navigation"  },
    { INTENT_SIMPLE_COMMAND,   SIMPLE_CMD_MEDIA,      "media"       },
    { INTENT_SIMPLE_COMMAND,   SIMPLE_CMD_VOLUME,     "volume"      },
    { INTENT_SIMPLE_COMMAND,   SIMPLE_CMD_EMERGENCY,  "emergency"   },
    { INTENT_SIMPLE_COMMAND,   SIMPLE_CMD_CAMERA,     "camera"      },
    { INTENT_COMPLEX_DIALOGUE, 0,                     "dialogue"    },
    { INTENT_TRANSLATION,      0,                     "translation" },
};
#define N_INTENTS (sizeof(INTENT_LABELS) / sizeof(INTENT_LABELS[0]))

/* ── Tokenizer (whitespace + subword stub) ─────────────────────────────── */

/* Simple bag-of-words tokenizer for the stub.
 * Production: replace with a proper multilingual tokenizer
 * (e.g., SentencePiece with the same vocab used during training). */
static void tokenize(const char *text, float *token_ids, int max_len, int *out_len) {
    /* TODO: implement multilingual subword tokenizer matching training vocab */
    /* Stub: hash each whitespace-separated word into [0, vocab_size) */
    char buf[INTENT_MAX_INPUT_LEN];
    strncpy(buf, text, sizeof(buf) - 1);
    buf[sizeof(buf) - 1] = '\0';

    int idx = 0;
    char *tok = strtok(buf, " \t\n");
    while (tok && idx < max_len) {
        /* djb2 hash mod vocab size (64k) */
        uint32_t h = 5381;
        for (const char *c = tok; *c; c++) h = ((h << 5) + h) + (uint8_t)*c;
        token_ids[idx++] = (float)(h % 65536);
        tok = strtok(NULL, " \t\n");
    }
    *out_len = idx;

    /* Zero-pad to max_len */
    for (int i = idx; i < max_len; i++) token_ids[i] = 0.0f;
}

/* ── Softmax ───────────────────────────────────────────────────────────── */

static void softmax(float *logits, int n) {
    float max_val = logits[0];
    for (int i = 1; i < n; i++) if (logits[i] > max_val) max_val = logits[i];
    float sum = 0.0f;
    for (int i = 0; i < n; i++) { logits[i] = expf(logits[i] - max_val); sum += logits[i]; }
    for (int i = 0; i < n; i++) logits[i] /= sum;
}

/* ── Public API ────────────────────────────────────────────────────────── */

int intent_pipeline_init(const char *model_path) {
    g_ort = OrtGetApiBase()->GetApi(ORT_API_VERSION);
    if (!g_ort) { fprintf(stderr, "[intent] ORT API unavailable\n"); return -1; }

    g_ort->CreateEnv(ORT_LOGGING_LEVEL_WARNING, "dashcam_intent", &g_env);
    g_ort->CreateSessionOptions(&g_opts);

    /* CPU-only inference — ONNX Runtime minimal build */
    g_ort->SetIntraOpNumThreads(g_opts, 2);
    g_ort->SetSessionGraphOptimizationLevel(g_opts, ORT_ENABLE_ALL);

    OrtStatus *status = g_ort->CreateSession(g_env, model_path, g_opts, &g_session);
    if (status) {
        fprintf(stderr, "[intent] CreateSession failed: %s\n",
                g_ort->GetErrorMessage(status));
        g_ort->ReleaseStatus(status);
        return -1;
    }

    g_ort->CreateCpuMemoryInfo(OrtArenaAllocator, OrtMemTypeDefault, &g_mem);
    fprintf(stderr, "[intent] loaded %s  classes=%zu\n", model_path, N_INTENTS);
    return 0;
}

int intent_pipeline_run(const char *transcript, const char *lang,
                        intent_result_t *result) {
    if (!g_session || !transcript || !result) return -1;

    /* Tokenize transcript into float token IDs */
    float token_ids[INTENT_MAX_INPUT_LEN];
    int   token_len = 0;
    tokenize(transcript, token_ids, INTENT_MAX_INPUT_LEN, &token_len);

    int64_t input_shape[] = { 1, INTENT_MAX_INPUT_LEN };
    OrtValue *input_tensor = NULL;
    g_ort->CreateTensorWithDataAsOrtValue(
        g_mem, token_ids, sizeof(token_ids),
        input_shape, 2, ONNX_TENSOR_ELEMENT_DATA_TYPE_FLOAT, &input_tensor);

    /* Run inference */
    const char *input_names[]  = { "input_ids" };
    const char *output_names[] = { "logits"    };
    OrtValue *output_tensor = NULL;

    OrtStatus *status = g_ort->Run(g_session, NULL,
                                   input_names,  (const OrtValue *const *)&input_tensor, 1,
                                   output_names, 1, &output_tensor);
    g_ort->ReleaseValue(input_tensor);

    if (status) {
        fprintf(stderr, "[intent] Run failed: %s\n", g_ort->GetErrorMessage(status));
        g_ort->ReleaseStatus(status);
        return -1;
    }

    /* Extract logits and apply softmax */
    float *logits = NULL;
    g_ort->GetTensorMutableData(output_tensor, (void **)&logits);

    float probs[N_INTENTS];
    memcpy(probs, logits, N_INTENTS * sizeof(float));
    softmax(probs, N_INTENTS);
    g_ort->ReleaseValue(output_tensor);

    /* Argmax */
    int best = 0;
    for (int i = 1; i < (int)N_INTENTS; i++)
        if (probs[i] > probs[best]) best = i;

    /* Low-confidence fallback to cloud */
    if (probs[best] < INTENT_CONF_THRESHOLD) {
        result->intent     = INTENT_COMPLEX_DIALOGUE;
        result->confidence = probs[best];
    } else {
        result->intent     = INTENT_LABELS[best].intent;
        result->simple_cmd = INTENT_LABELS[best].cmd;
        result->confidence = probs[best];
    }

    strncpy(result->detected_lang, lang, sizeof(result->detected_lang) - 1);
    return 0;
}

void intent_pipeline_destroy(void) {
    if (g_session) { g_ort->ReleaseSession(g_session);       g_session = NULL; }
    if (g_opts)    { g_ort->ReleaseSessionOptions(g_opts);   g_opts    = NULL; }
    if (g_mem)     { g_ort->ReleaseMemoryInfo(g_mem);        g_mem     = NULL; }
    if (g_env)     { g_ort->ReleaseEnv(g_env);               g_env     = NULL; }
}

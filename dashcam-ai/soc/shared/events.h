#pragma once
#include <stdint.h>

/* Canonical event types published on the event bus.
 * All daemons include this header — no daemon defines its own event types. */

typedef enum {
    /* media-daemon */
    EVT_VOICE_ACTIVITY_START    = 0x01,
    EVT_VOICE_ACTIVITY_END      = 0x02,
    EVT_COLLISION_PREROLL_TAG   = 0x03,   /* tag pre-event buffer for preservation */

    /* ai-daemon */
    EVT_COLLISION_DETECTED      = 0x10,
    EVT_OBJECT_DETECTED         = 0x11,
    EVT_TRANSCRIPT_READY        = 0x12,
    EVT_INTENT_CLASSIFIED       = 0x13,
    EVT_WAKE_WORD_DETECTED      = 0x14,

    /* cloud-daemon */
    EVT_LTE_CONNECTED           = 0x20,
    EVT_LTE_DISCONNECTED        = 0x21,
    EVT_LLM_RESPONSE_READY      = 0x22,
    EVT_UPLOAD_COMPLETE         = 0x23,

    /* power-daemon */
    EVT_SYSTEM_DRIVING          = 0x30,
    EVT_SYSTEM_PARKED           = 0x31,
    EVT_SYSTEM_RESUMED          = 0x32,
    EVT_SUSPEND_REQUESTED       = 0x33,
    EVT_SUSPEND_ACK             = 0x34,
} event_type_t;

typedef enum {
    WAKE_REASON_GSENSOR = 0,
    WAKE_REASON_KWS     = 1,
    WAKE_REASON_SCHEDULED = 2,
} wake_reason_t;

typedef enum {
    INTENT_SIMPLE_COMMAND   = 0,   /* edge-routable, offline capable */
    INTENT_COMPLEX_DIALOGUE = 1,   /* requires Cloud LLM */
    INTENT_TRANSLATION      = 2,   /* requires Cloud LLM — core feature */
} intent_type_t;

typedef struct {
    event_type_t type;
    uint64_t     timestamp_us;
    union {
        struct { char transcript[256]; char lang[8]; intent_type_t intent; } intent;
        struct { float confidence; uint32_t clip_id; }                       collision;
        struct { wake_reason_t reason; }                                      wake;
        struct { char response[1024]; char lang[8]; }                        llm;
    } data;
} dashcam_event_t;

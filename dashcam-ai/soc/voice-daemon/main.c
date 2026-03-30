#include <stdio.h>
#include "../shared/event_bus.h"
#include "router.h"
#include "session.h"
#include "tts.h"

/* voice-daemon — voice pipeline orchestrator
 *
 * Receives EVT_INTENT_CLASSIFIED from ai-daemon.
 * Routes to edge response (simple commands, offline) or
 * cloud-daemon (complex dialogue / multilingual translation).
 * Manages multi-turn session context.
 * Runs Piper TTS for edge responses.
 *
 * Latency targets:
 *   Edge path  : < 400ms  (STT already done in ai-daemon)
 *   Cloud path : 800ms–1.2s (LTE round-trip + LLM + TTS)
 */

static session_t *session;

static void on_intent_classified(const dashcam_event_t *evt, void *ctx) {
    const char *transcript = evt->data.intent.transcript;
    const char *lang       = evt->data.intent.lang;
    intent_type_t intent   = evt->data.intent.intent;

    session_update(session, transcript, lang);

    if (intent == INTENT_SIMPLE_COMMAND) {
        /* Edge path — fully offline */
        char response[512];
        router_handle_edge(transcript, lang, session, response, sizeof(response));
        tts_speak(response, lang);
    } else {
        /* Cloud path — INTENT_COMPLEX_DIALOGUE or INTENT_TRANSLATION */
        int sent = router_send_to_cloud(transcript, lang, session);
        if (!sent) {
            /* LTE unavailable — queue and notify user */
            tts_speak("I'll respond when you're back in range.", "en");
            /* TODO: enqueue to storage-daemon offline queue */
        }
    }
}

static void on_llm_response(const dashcam_event_t *evt, void *ctx) {
    tts_speak(evt->data.llm.response, evt->data.llm.lang);
    session_update(session, evt->data.llm.response, evt->data.llm.lang);
}

int main(void) {
    session = session_create();
    int bus_fd = event_bus_connect();

    /* TODO: tts_init("models/piper-tts.onnx"); */

    event_bus_subscribe(bus_fd, EVT_INTENT_CLASSIFIED, on_intent_classified, NULL);
    event_bus_subscribe(bus_fd, EVT_LLM_RESPONSE_READY, on_llm_response, NULL);

    event_bus_dispatch(bus_fd);   /* blocks */

    session_destroy(session);
    event_bus_disconnect(bus_fd);
    return 0;
}

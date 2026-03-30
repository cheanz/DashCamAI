#include <stdio.h>
#include "../shared/event_bus.h"
#include "lte_manager.h"
#include "llm_client.h"
#include "s3_client.h"
#include "queue_flush.h"

/* cloud-daemon — owns all network I/O
 *
 * The only daemon that touches the LTE module.
 * Publishes EVT_LTE_CONNECTED / EVT_LTE_DISCONNECTED on state changes.
 * Exposes Unix socket RPC for voice-daemon LLM requests (synchronous).
 * Receives async clip upload jobs from storage-daemon.
 * Flushes offline queue (transcripts + clips) on LTE reconnect.
 */

static void on_lte_state_change(lte_state_t state, void *ctx) {
    int bus_fd = *(int *)ctx;
    dashcam_event_t evt = {
        .type         = (state == LTE_CONNECTED) ? EVT_LTE_CONNECTED : EVT_LTE_DISCONNECTED,
        .timestamp_us = /* TODO: get monotonic time */ 0,
    };
    event_bus_publish(bus_fd, &evt);

    if (state == LTE_CONNECTED) {
        queue_flush_trigger();   /* drain offline queue on reconnect */
    }
}

int main(void) {
    int bus_fd = event_bus_connect();

    /* TODO: lte_manager_init(on_lte_state_change, &bus_fd); */
    /* TODO: llm_client_init(getenv("LLM_API_ENDPOINT"), getenv("LLM_API_KEY")); */
    /* TODO: s3_client_init(getenv("S3_BUCKET"), getenv("AWS_REGION")); */
    /* TODO: queue_flush_init("config/storage-policy.yaml"); */

    /* TODO: start LLM RPC server thread (Unix socket, synchronous) */
    /* TODO: start S3 upload worker thread (async job queue)         */
    /* TODO: start queue flush thread                                 */

    event_bus_dispatch(bus_fd);   /* blocks */

    event_bus_disconnect(bus_fd);
    return 0;
}

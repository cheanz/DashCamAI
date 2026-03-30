#include <stdio.h>
#include "../shared/event_bus.h"
#include "loop_manager.h"
#include "clip_store.h"
#include "offline_queue.h"

/* storage-daemon — eMMC gatekeeper
 *
 * Arbitrates all writes to eMMC. No other daemon writes directly to disk.
 *
 * Eviction priority (highest → lowest):
 *   1. Evidence clips      — never evicted automatically
 *   2. Queued transcripts  — evicted after TTL (default 7 days)
 *   3. Oldest loop footage — evicted first when storage pressure applies
 */

static void on_collision_detected(const dashcam_event_t *evt, void *ctx) {
    clip_store_preserve(evt->data.collision.clip_id);
}

static void on_lte_connected(const dashcam_event_t *evt, void *ctx) {
    /* Notify cloud-daemon of pending clips via upload job queue */
    clip_store_enqueue_pending_uploads();
    offline_queue_notify_flush_ready();
}

int main(void) {
    int bus_fd = event_bus_connect();

    /* TODO: loop_manager_init("config/storage-policy.yaml"); */
    /* TODO: clip_store_init();     */
    /* TODO: offline_queue_init();  */

    event_bus_subscribe(bus_fd, EVT_COLLISION_DETECTED,  on_collision_detected, NULL);
    event_bus_subscribe(bus_fd, EVT_COLLISION_PREROLL_TAG, on_collision_detected, NULL);
    event_bus_subscribe(bus_fd, EVT_LTE_CONNECTED,       on_lte_connected, NULL);
    event_bus_subscribe(bus_fd, EVT_SUSPEND_REQUESTED,
        /* flush and checkpoint before suspend */ NULL, NULL);

    event_bus_dispatch(bus_fd);   /* blocks */

    event_bus_disconnect(bus_fd);
    return 0;
}

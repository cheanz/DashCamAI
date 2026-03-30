#include <stdio.h>
#include <unistd.h>
#include "../shared/event_bus.h"
#include "state_machine.h"
#include "suspend.h"

/* power-daemon — SoC-side power state machine
 *
 * Mirrors MCU state. Receives state notifications from MCU via UART (comm_task).
 * Coordinates graceful suspend: signals all daemons → collects ACKs → calls suspend.
 * On resume, publishes EVT_SYSTEM_RESUMED with wake reason.
 */

static int bus_fd;

static void on_suspend_ack(const dashcam_event_t *evt, void *ctx) {
    suspend_record_ack();
    if (suspend_all_acked()) {
        suspend_execute();   /* calls systemctl suspend */
    }
}

static void on_mcu_state_parked(void) {
    dashcam_event_t evt = { .type = EVT_SUSPEND_REQUESTED };
    event_bus_publish(bus_fd, &evt);
    suspend_await_acks(/* timeout_ms */ 3000);
}

static void on_mcu_state_driving(void) {
    dashcam_event_t evt = { .type = EVT_SYSTEM_DRIVING };
    event_bus_publish(bus_fd, &evt);
}

int main(void) {
    bus_fd = event_bus_connect();

    /* TODO: open UART to MCU */
    /* TODO: state_machine_init(on_mcu_state_driving, on_mcu_state_parked); */
    /* TODO: on resume, publish EVT_SYSTEM_RESUMED with wake_reason from MCU */

    event_bus_subscribe(bus_fd, EVT_SUSPEND_ACK, on_suspend_ack, NULL);

    event_bus_dispatch(bus_fd);   /* blocks */

    event_bus_disconnect(bus_fd);
    return 0;
}

#include "comm_task.h"
#include "power_task.h"
#include "FreeRTOS.h"
#include "task.h"

#define UART_BAUD_RATE   115200

void comm_task(void *pvParameters) {
    /* TODO: initialize UART peripheral at UART_BAUD_RATE */

    for (;;) {
        /* TODO: read incoming frame from SoC */
        /* Frame types expected from SoC:                          */
        /*   MSG_SUSPEND_READY   — SoC daemons flushed, safe to suspend */
        /*   MSG_WAKE_ACK        — SoC acknowledged GPIO wake           */
        /*   MSG_STATE_QUERY     — SoC requesting current MCU state      */

        /* TODO: on MSG_WAKE_ACK, release SoC wake GPIO via power_task  */

        /* TODO: transmit outgoing frames to SoC                        */
        /* Frame types sent to SoC:                                     */
        /*   MSG_STATE_DRIVING   — ignition on, debounce cleared         */
        /*   MSG_STATE_PARKED    — debounce elapsed, entering suspend     */
        /*   MSG_WAKE_REASON     — gsensor or KWS triggered wake         */

        vTaskDelay(pdMS_TO_TICKS(10));
    }
}

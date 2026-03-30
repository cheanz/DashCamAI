#include "kws_task.h"
#include "FreeRTOS.h"
#include "task.h"

/* KWS is gated: only runs when power_task sends a task notification */
void kws_task(void *pvParameters) {
    wake_event_t event = { .reason = WAKE_REASON_KWS };

    for (;;) {
        /* Block until power_task enables KWS (parked state entry) */
        ulTaskNotifyTake(pdTRUE, portMAX_DELAY);

        /* TODO: initialize MCU-side KWS model (kws-parked.bin) */
        /* TODO: open ADC / PDM microphone for audio capture */

        /* Run KWS inference loop until driving state resumes */
        while (/* parked */ 1) {
            /* TODO: capture audio frame */
            /* TODO: run KWS inference */
            /* if (kws_result == KEYWORD_DETECTED) { */
            /*     event.timestamp_ms = xTaskGetTickCount() * portTICK_PERIOD_MS; */
            /*     xQueueSend(wake_event_queue, &event, 0); */
            /* } */

            /* Yield to allow other tasks to run */
            taskYIELD();
        }
    }
}

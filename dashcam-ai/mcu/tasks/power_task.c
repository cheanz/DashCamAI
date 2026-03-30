#include "power_task.h"
#include "gsensor_task.h"
#include "kws_task.h"
#include "FreeRTOS.h"
#include "task.h"
#include "queue.h"

#define PARKED_DEBOUNCE_MS   60000   /* 60s — prevents thrash in stop-and-go */
#define SOC_WAKE_GPIO        /* TODO: define board GPIO pin */

extern TaskHandle_t kws_task_handle;

static void set_soc_wake_gpio(int high) {
    /* TODO: gpio_set_level(SOC_WAKE_GPIO, high); */
}

void power_task(void *pvParameters) {
    power_state_t  state = POWER_STATE_DRIVING;
    wake_event_t   event;
    TickType_t     last_motion_tick = xTaskGetTickCount();

    for (;;) {
        /* Check ignition / motion signal to detect driving→parked */
        /* TODO: bool ignition_on = gpio_get_level(IGNITION_GPIO); */
        bool ignition_on = true; /* placeholder */

        if (ignition_on) {
            last_motion_tick = xTaskGetTickCount();
            if (state == POWER_STATE_PARKED || state == POWER_STATE_PARKED_ALERT) {
                state = POWER_STATE_DRIVING;
                /* Disable KWS — not needed while driving */
                /* kws is gated by task notification; no notification = stays blocked */
            }
        } else {
            TickType_t idle_ms = (xTaskGetTickCount() - last_motion_tick) * portTICK_PERIOD_MS;
            if (state == POWER_STATE_DRIVING && idle_ms >= PARKED_DEBOUNCE_MS) {
                state = POWER_STATE_PARKED;
                /* Enable KWS */
                xTaskNotifyGive(kws_task_handle);
            }
        }

        /* Drain wake events */
        if (xQueueReceive(wake_event_queue, &event, 0) == pdTRUE) {
            if (state == POWER_STATE_PARKED) {
                state = POWER_STATE_PARKED_ALERT;
                set_soc_wake_gpio(1);
                /* SoC will ACK via comm_task; GPIO released after ACK */
            }
        }

        vTaskDelay(pdMS_TO_TICKS(100));
    }
}

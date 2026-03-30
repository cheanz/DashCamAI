#include "gsensor_task.h"
#include "FreeRTOS.h"
#include "task.h"

/* TODO: replace with board-specific G-sensor driver header */
/* #include "driver/i2c.h" */

#define GSENSOR_POLL_MS          10
#define IMPACT_THRESHOLD_MG      250   /* configurable via config/gsensor-thresholds.yaml */

void gsensor_task(void *pvParameters) {
    wake_event_t event = { .reason = WAKE_REASON_GSENSOR };

    for (;;) {
        /* TODO: read acceleration vector from G-sensor over I2C/SPI */
        /* int16_t ax, ay, az; gsensor_read(&ax, &ay, &az); */

        /* TODO: compute resultant magnitude and compare to threshold */
        /* uint32_t magnitude = sqrt(ax*ax + ay*ay + az*az); */
        /* if (magnitude > IMPACT_THRESHOLD_MG) { */
        /*     event.timestamp_ms = xTaskGetTickCount() * portTICK_PERIOD_MS; */
        /*     xQueueSend(wake_event_queue, &event, 0); */
        /* } */

        vTaskDelay(pdMS_TO_TICKS(GSENSOR_POLL_MS));
    }
}

#include "FreeRTOS.h"
#include "task.h"
#include "queue.h"

#include "tasks/gsensor_task.h"
#include "tasks/kws_task.h"
#include "tasks/power_task.h"
#include "tasks/comm_task.h"

/* Shared wake event queue — produced by gsensor/kws, consumed by power_task */
QueueHandle_t wake_event_queue;

void app_main(void) {
    wake_event_queue = xQueueCreate(8, sizeof(wake_event_t));
    configASSERT(wake_event_queue);

    xTaskCreate(power_task,   "power",   2048, NULL, 5, NULL);
    xTaskCreate(gsensor_task, "gsensor", 1024, NULL, 5, NULL);
    xTaskCreate(kws_task,     "kws",     4096, NULL, 3, NULL);
    xTaskCreate(comm_task,    "comm",    2048, NULL, 2, NULL);

    vTaskStartScheduler();
    /* Should never reach here */
    for (;;);
}

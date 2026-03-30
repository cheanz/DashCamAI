cmake_minimum_required(VERSION 3.16)
project(dashcam-ai C CXX)

set(CMAKE_C_STANDARD 11)
set(CMAKE_CXX_STANDARD 17)

# MCU firmware is built separately via its own toolchain file.
# See mcu/CMakeLists.txt for the FreeRTOS / ESP-IDF / STM32 build.

# SoC Linux daemons
add_subdirectory(soc/shared)
add_subdirectory(soc/media-daemon)
add_subdirectory(soc/ai-daemon)
add_subdirectory(soc/voice-daemon)
add_subdirectory(soc/cloud-daemon)
add_subdirectory(soc/storage-daemon)
add_subdirectory(soc/power-daemon)

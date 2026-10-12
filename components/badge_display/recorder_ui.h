#pragma once
#include <M5Unified.h>
#include "recorder_controls.h"
extern "C" void badge_recorder_set(uint8_t phase, uint32_t captured_seconds, uint32_t saved_seconds, const char* message);
extern "C" uint8_t badge_recorder_take_action();
extern "C" bool badge_recorder_cancelled();
extern "C" void badge_recorder_test_prepare();
void recorder_arm(uint8_t kind);
RecorderOverlay recorder_overlay(M5Canvas& frame);

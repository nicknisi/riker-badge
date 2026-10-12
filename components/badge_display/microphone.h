#pragma once
#include <stdint.h>
#include <M5Unified.h>

// The USB command only arms the test. A physical A press starts capture;
// B/touch cancels. These functions never run at boot or from network input.
extern "C" int badge_mic_capture(int16_t* samples, uint32_t count);
extern "C" int badge_mic_cancelled();
extern "C" uint32_t badge_mic_duration_ms();
extern "C" void badge_mic_finish();
extern "C" int badge_audio_claim();
extern "C" void badge_audio_release();
extern "C" int badge_usb_line(const uint8_t* line, uint32_t length);
bool badge_mic_overlay(uint32_t ms, M5Canvas& frame);

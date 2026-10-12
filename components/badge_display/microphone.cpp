#include <M5Unified.h>
#include <atomic>
#include <stdio.h>
#include "esp_timer.h"
#include "microphone.h"

namespace {
std::atomic<unsigned> phase{0}; // 0 idle, 1 armed, 2 countdown, 3 mic on, 4 USB export
std::atomic<unsigned> activity{0}; // single arbiter: 0 free, 1 microphone, 2 Wi-Fi setup
std::atomic<bool> start_requested{false}, cancelled{false}, warming{false};
std::atomic<uint32_t> phase_at{0}, captured_ms{0};
uint32_t now_ms() { return static_cast<uint32_t>(esp_timer_get_time() / 1000); }
void sleep_tick() { vTaskDelay(pdMS_TO_TICKS(10)); }

bool drain(uint32_t timeout) {
    const auto started = now_ms();
    while (M5.Mic.isRecording()) {
        if (cancelled.load() || now_ms() - started > timeout) return false;
        sleep_tick();
    }
    return !cancelled.load();
}
}

extern "C" int badge_mic_capture(int16_t* samples, uint32_t count) {
    if (!samples || count != 480000 || !badge_audio_claim()) return -1;
    start_requested.store(false);
    cancelled.store(false);
    captured_ms.store(0);
    phase_at.store(now_ms());
    phase.store(1);
    while (!start_requested.load() && !cancelled.load() && now_ms() - phase_at.load() < 60000) sleep_tick();
    if (!start_requested.load() || cancelled.load()) { badge_mic_finish(); return -2; }
    phase_at.store(now_ms());
    phase.store(2);
    while (!cancelled.load() && now_ms() - phase_at.load() < 3000) sleep_tick();
    if (cancelled.load()) { badge_mic_finish(); return -2; }

    // M5.begin configured the board pins only. I2S and codec start here,
    // after the user's physical A press and visible countdown.
    warming.store(true);
    phase.store(3);
    bool success = M5.Mic.begin();
    // Discard one second of codec warm-up, without power-cycling between buffers.
    if (success) success = M5.Mic.record(samples, 16000, 16000, false) && drain(4000);
    if (success) {
        phase_at.store(now_ms());
        warming.store(false);
        success = M5.Mic.record(samples, count, 16000, false);
        if (success) {
            printf("MIC_RECORDING\n");
            success = drain(40000);
            captured_ms.store(now_ms() - phase_at.load());
        }
    }
    M5.Mic.end(); // stop the capture task/codec on completion AND every error/cancel path
    warming.store(false);
    if (!success || cancelled.load()) { badge_mic_finish(); return -3; }
    phase.store(4);
    return 0;
}

extern "C" int badge_mic_cancelled() { return cancelled.load(); }
extern "C" uint32_t badge_mic_duration_ms() { return captured_ms.load(); }
extern "C" int badge_audio_claim() { unsigned expected=0; return activity.compare_exchange_strong(expected,1); }
extern "C" void badge_audio_release() { unsigned expected=1; activity.compare_exchange_strong(expected,0); }
extern "C" void badge_mic_finish() { phase.store(0); badge_audio_release(); }
extern "C" int badge_mic_is_busy() { return activity.load() == 1; }
extern "C" int badge_try_wifi() { unsigned expected=0; return activity.compare_exchange_strong(expected, 2); }
extern "C" void badge_end_wifi() { unsigned expected=2; activity.compare_exchange_strong(expected, 0); }

extern "C" int badge_usb_line(const uint8_t* line, uint32_t length) {
    // Serialize each framed line with ESP-IDF/C stdio logs, so log traffic
    // cannot splice into the middle of a sample's hex payload.
    flockfile(stdout);
    int result = fwrite(line, 1, length, stdout) == length ? 0 : -1;
    if (fputc('\n', stdout) == EOF || fflush(stdout) != 0) result = -1;
    funlockfile(stdout);
    return result;
}

bool badge_mic_overlay(uint32_t ms, M5Canvas& frame) {
    const auto current = phase.load();
    static unsigned previous = 0;
    static uint32_t previous_second = 0;
    if (!current) { previous = 0; return false; }
    if (M5.BtnB.wasPressed() || M5.Touch.getDetail().wasPressed()) cancelled.store(true);
    if (current == 1 && M5.BtnA.wasPressed()) start_requested.store(true);
    const uint32_t elapsed = (ms - phase_at.load()) / 1000;
    if (previous == current && previous_second == elapsed && !cancelled.load()) return true;
    // Stop the motor while merely armed, before the physical start/countdown.
    // During capture the GUI only polls touch/buttons and draws over SPI.
    // StopWatch M5.update does not touch Mic; shared I2C transactions are port-locked by LGFX.
    if (previous == 0) M5.Power.setVibration(0);
    previous = current;
    previous_second = elapsed;
    M5.Display.setBrightness(160);
    frame.fillScreen(TFT_BLACK);
    frame.setTextColor(TFT_WHITE, TFT_BLACK);
    frame.setFont(&fonts::FreeSansBold24pt7b);
    frame.drawString(current == 3 ? "RECORDING" : "MIC TEST", 233, 109);
    if (current == 3) frame.fillCircle(233, 163, 12, TFT_RED);
    frame.setFont(&fonts::FreeSansBold18pt7b);
    const char* message = "";
    char timer[16];
    if (cancelled.load()) message = "Cancelling...";
    else if (current == 1) message = "Orange: record 30 sec";
    else if (current == 2) { snprintf(timer, sizeof(timer), "%u", elapsed < 3 ? 3-elapsed : 0); message = timer; }
    else if (current == 3 && warming.load()) message = "Warming mic...";
    else if (current == 3) { snprintf(timer, sizeof(timer), "00:%02u", elapsed < 30 ? 30-elapsed : 0); message = timer; }
    else message = "Sending to USB";
    frame.drawString(message, 233, 244);
    frame.setFont(&fonts::Font2);
    frame.drawString(current == 1 ? "Only start with everyone's consent." : (current == 4 ? "Microphone is OFF." : ""), 233, 324);
    frame.drawString("Blue or tap: cancel", 233, 382);
    frame.pushSprite(0, 0);
    return true;
}

#include <M5Unified.h>
#include <atomic>
#include "microphone.h"
extern "C" bool badge_recorder_cancelled();

namespace {
using Sink = bool (*)(void*, const int16_t*, uint32_t);
std::atomic<bool> running{false}, stopping{false}, overflow{false}, warm{false}, failed{false};
Sink sink = nullptr;
void* context = nullptr;
bool synthetic = false;
int16_t frames[2][320]; // 20 ms each; I2S DMA copies into these internal buffers.
unsigned warm_frames = 0;
void released(void*, void* data, size_t length) {
    if (stopping.load()) return;
    if (warm_frames) --warm_frames;
    else {
        warm.store(false);
        if (length != 320 || !sink(context, static_cast<int16_t*>(data), length)) {
            overflow.store(true); stopping.store(true); return;
        }
    }
    if (!stopping.load() && !M5.Mic.record(static_cast<int16_t*>(data), length, 16000, false)) {
        failed.store(true); stopping.store(true);
    }
}
void control(void*) {
    if (synthetic) {
        // This test path never starts the microphone. The same sink/ring is used.
        if (M5.Mic.isRunning()) {M5.Mic.end();failed.store(true);}
        auto last=xTaskGetTickCount();
        while (!stopping.load() && !badge_recorder_cancelled() && !failed.load()) {
            vTaskDelayUntil(&last,pdMS_TO_TICKS(20));
            if (!sink(context,frames[0],320)) {overflow.store(true);stopping.store(true);}
        }
    } else {
        warm_frames=50; warm.store(true);
        if (badge_recorder_cancelled()) stopping.store(true);
        if (!stopping.load() && !M5.Mic.begin()) failed.store(true);
        if (!failed.load() && !stopping.load()) {
            M5.Mic.setBufferReleaseCallback(nullptr,released);
            if (!M5.Mic.record(frames[0],320,16000,false) || !M5.Mic.record(frames[1],320,16000,false)) failed.store(true);
        }
        while (!stopping.load() && !badge_recorder_cancelled() && !failed.load()) vTaskDelay(pdMS_TO_TICKS(10));
        M5.Mic.end();
        M5.Mic.setBufferReleaseCallback(nullptr,nullptr);
    }
    warm.store(false);
    // Callback completion is guaranteed before the sink/context can be released.
    sink=nullptr;context=nullptr;
    badge_audio_release();
    running.store(false,std::memory_order_release);
    vTaskDelete(nullptr);
}
}
extern "C" int badge_stream_begin(Sink callback, void* data, bool test_source) {
    if (running.load() || !callback || badge_recorder_cancelled() || !badge_audio_claim()) return -1;
    sink=callback;context=data;synthetic=test_source;
    memset(frames,0,sizeof(frames));
    stopping.store(false);overflow.store(false);failed.store(false);warm.store(!test_source);running.store(true);
    if (xTaskCreate(control,"audio-control",6144,nullptr,4,nullptr)!=pdPASS) {
        running.store(false);badge_audio_release();return -2;
    }
    return 0;
}
extern "C" void badge_stream_request_stop() {stopping.store(true);}
extern "C" bool badge_stream_running() {return running.load(std::memory_order_acquire);}
extern "C" bool badge_stream_warming() {return warm.load();}
extern "C" bool badge_stream_failed() {return failed.load();}
extern "C" bool badge_stream_overflowed() {return overflow.load();}
extern "C" bool badge_stream_mic_active() {return M5.Mic.isRunning();}

#pragma once
#include <stdint.h>
enum class RecorderOverlay { Hidden, Visible, Settings };
constexpr RecorderOverlay recorder_visibility(uint8_t phase, bool mic, bool orange, bool blue) {
    if (!phase) return RecorderOverlay::Hidden;
    if ((phase==5 || phase==7 || phase==10) && !mic && orange && blue) return RecorderOverlay::Settings;
    return RecorderOverlay::Visible;
}
constexpr bool recorder_can_discard(uint8_t phase, bool orange) {
    return (phase==5 || phase==7) && !orange;
}

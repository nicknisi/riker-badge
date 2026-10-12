#include <M5Unified.h>
#include <algorithm>
#include <cmath>
#include <stdio.h>
#include "nick_frames.h"
#include "microphone.h"
#include "recorder_ui.h"

// Nick is the little operator waiting for Riker, not a fake progress indicator.
static constexpr char LINKEDIN_URL[] = "https://linkedin.com/in/nicknisi";
static constexpr int QR_SIZE = 320;
static constexpr uint8_t QR_VERSION = 3;
static constexpr uint16_t CREAM = 0xffbd, MINT = 0x9f33, AMBER = 0xfd89;
static M5Canvas frame(&M5.Display);
static bool name_mode = false, qr_mode = false, calm = false, dirty = true;
static bool celebrating = false, vibrating = false;
static uint8_t last_state = 255;
static uint32_t celebration_at = 0, reaction_at = 0, vibration_at = 0, boops = 0;
static const char* reaction = nullptr;
static float nudge_x = 0;
static uint8_t wifi_phase = 0, wifi_action = 0;
static bool wifi_dirty = true, wifi_web_qr = false;
static bool both_latched = false, release_gate = false;
static bool settings_open=false, settings_dirty=true;
static unsigned settings_index=0;
static void render_settings() {
    if(!settings_dirty)return;
    frame.fillScreen(TFT_BLACK);frame.setTextColor(CREAM,TFT_BLACK);
    frame.setFont(&fonts::FreeSansBold18pt7b);frame.drawString("Settings",233,58);
    const char* labels[]={"Wi-Fi setup","Record meeting","Record a take","Back"};
    for(unsigned i=0;i<4;i++){
        const bool selected=i==settings_index;const int y=121+i*70;
        frame.fillRoundRect(76,y,314,52,8,selected?MINT:0x2125);
        frame.setTextColor(selected?TFT_BLACK:CREAM,selected?MINT:0x2125);
        frame.setFont(&fonts::FreeSansBold12pt7b);frame.drawString(labels[i],233,y+26);
    }
    frame.setFont(&fonts::Font2);frame.setTextColor(CREAM,TFT_BLACK);
    frame.drawString("Orange: next / Blue: choose",233,434);
    frame.pushSprite(0,0);settings_dirty=false;
}
static char wifi_join_qr[160] = {}, wifi_message[128] = {};
static constexpr int WIFI_QR_SIZE = 280;
static constexpr uint8_t WIFI_QR_VERSION = 4;
extern "C" uint8_t badge_wifi_take_action() { auto action=wifi_action; wifi_action=0; return action; }
extern "C" void badge_wifi_set(uint8_t phase, const char* qr, const char* message) {
    if (phase != wifi_phase || strcmp(qr,wifi_join_qr) || strcmp(message,wifi_message)) wifi_dirty=true;
    if (wifi_phase == 0 && phase) wifi_web_qr=false;
    wifi_phase=phase;
    if (!phase) { memset(wifi_join_qr,0,sizeof(wifi_join_qr)); memset(wifi_message,0,sizeof(wifi_message)); return; }
    snprintf(wifi_join_qr,sizeof(wifi_join_qr),"%s",qr);
    snprintf(wifi_message,sizeof(wifi_message),"%s",message);
}
static void render_wifi() {
    if (!wifi_dirty) return;
    M5.Display.setBrightness(180);
    frame.fillScreen(TFT_BLACK); frame.setTextColor(CREAM,TFT_BLACK);
    frame.setFont(&fonts::FreeSansBold18pt7b); frame.drawString("Wi-Fi setup",233,45);
    frame.setFont(&fonts::Font2);
    frame.drawString(wifi_web_qr ? "2. Scan to open the phone form" : "1. Scan to join setup Wi-Fi",233,85);
    bool qr_ready=false;
    if (wifi_phase == 2) {
        const unsigned version=wifi_web_qr?2:WIFI_QR_VERSION;
        frame.qrcode(wifi_web_qr ? "http://192.168.4.1/" : wifi_join_qr,93,105,WIFI_QR_SIZE,version,true);
        // Check the actual sprite's quiet zone and three finder patterns, not just AP startup.
        const int modules=17+4*version;
        int scale=WIFI_QR_SIZE/modules;
        if((WIFI_QR_SIZE-modules*scale)/2<scale*4)scale=(WIFI_QR_SIZE-scale*8)/modules;
        const int offset=(WIFI_QR_SIZE-modules*scale)/2;
        qr_ready=frame.readPixel(93,105)!=0;
        for(int corner=0;corner<3;++corner)for(int y=0;y<7;++y)for(int x=0;x<7;++x){
            const bool black=x==0 || x==6 || y==0 || y==6 || (x>=2 && x<=4 && y>=2 && y<=4);
            const int mx=x+(corner==1?modules-7:0),my=y+(corner==2?modules-7:0);
            qr_ready &= (frame.readPixel(93+offset+mx*scale+scale/2,105+offset+my*scale+scale/2)==0)==black;
        }
    }
    else { frame.setFont(&fonts::FreeSansBold12pt7b); frame.drawString("One moment...",233,233); }
    frame.setFont(&fonts::Font2); frame.drawString(wifi_message,233,402);
    frame.drawString("Orange: next  Blue: exit",233,429);
    frame.pushSprite(0,0); wifi_dirty=false;
    // Never log the QR payload: it contains the temporary AP password.
    printf("WIFI_DISPLAY phase=%u qr=%u\n",wifi_phase,qr_ready);
}

static void pulse(uint32_t ms) {
    if (calm || qr_mode) return;
    M5.Power.setVibration(90);
    vibration_at = ms;
    vibrating = true;
}

static void avatar(const NickAnimation& animation, int center_x, int top, int scale, uint32_t ms) {
    unsigned tick = (calm ? 0 : ms) % animation.duration, index = 0;
    while (index + 1 < animation.count && tick >= animation.delays[index]) tick -= animation.delays[index++];
    const auto* pixels = animation.pixels + animation.indices[index] * animation.width * animation.height;
    // Native nearest-neighbor blit: integer scale and original GIF timing.
    frame.pushImageRotateZoom(center_x, top + animation.height * scale / 2,
        animation.width / 2.0f, animation.height / 2.0f, 0, scale, scale,
        animation.width, animation.height, pixels);
}

static void sparkle(uint32_t elapsed) {
    const float seconds = elapsed / 1000.0f;
    const uint16_t colors[] = {MINT, AMBER, CREAM, 0xfbb8};
    for (int i = 0; i < 12; ++i) {
        const float vx = ((i % 5) - 2) * 45.0f;
        const float vy = -100.0f - (i % 3) * 12.0f;
        const int x = 233 + static_cast<int>(vx * seconds);
        const int y = 170 + static_cast<int>(vy * seconds + 95 * seconds * seconds);
        frame.fillRect(x, y, 4, 4, colors[i % 4]);
    }
}

static void render_qr() {
    frame.fillScreen(TFT_WHITE);
    frame.setTextColor(TFT_BLACK, TFT_WHITE);
    frame.setFont(&fonts::FreeSansBold12pt7b);
    frame.drawString("LET'S CONNECT", 233, 46);
    // The square fits inside the visible circle; margin=true preserves >=4 quiet modules.
    frame.qrcode(LINKEDIN_URL, (466 - QR_SIZE) / 2, (466 - QR_SIZE) / 2, QR_SIZE, QR_VERSION, true);
    frame.setFont(&fonts::Font2);
    frame.drawString("linkedin.com/in/nicknisi", 233, 413);
    frame.drawString("tap to return", 233, 438);
    frame.pushSprite(0, 0);
    printf("BADGE_VIEW qr\n");
}

extern "C" int badge_begin() {
    auto config = M5.config();
    config.fallback_board = m5::board_t::board_M5StopWatch;
    config.internal_mic = true; // board config only; capture requires explicit physical confirmation
    config.internal_spk = false;
    M5.begin(config);
    printf("BADGE_MIC_IDLE configured=%u running=%u\n", M5.Mic.isEnabled(), M5.Mic.isRunning());
    if (M5.Mic.isRunning()) { M5.Mic.end(); return -3; }
    printf("BADGE_PANEL board=%u width=%d height=%d\n", static_cast<unsigned>(M5.getBoard()), M5.Display.width(), M5.Display.height());
    // The visible AMOLED is 466px; the current CO5300 driver exposes 468px.
    if (M5.getBoard() != m5::board_t::board_M5StopWatch || M5.Display.width() < 466 || M5.Display.height() < 466) return -1;
    M5.Display.setBrightness(160);
    M5.BtnB.setHoldThresh(700);
    frame.setPsram(true);
    frame.setColorDepth(16);
    frame.setSwapBytes(true); // generated uint16_t values are native RGB565, not byte-swapped
    if (!frame.createSprite(466, 466)) return -2;
    frame.setTextDatum(middle_center);
    printf("BADGE_RENDER_READY\n");
    return 0;
}

extern "C" void badge_update(uint8_t state, const char* detail, uint32_t ms) {
    M5.update();
    if (vibrating && ms - vibration_at >= 60) { M5.Power.setVibration(0); vibrating=false; }
    static bool mic_was_active = false, wifi_was_active = false;
    const bool both = M5.BtnA.isPressed() && M5.BtnB.isPressed();
    if (badge_mic_overlay(ms, frame)) {
        both_latched=both;
        if (M5.BtnA.isPressed() || M5.BtnB.isPressed()) release_gate=true;
        if (vibrating) { M5.Power.setVibration(0); vibrating = false; }
        celebrating = false;
        reaction = nullptr;
        mic_was_active = true;
        return;
    }
    if (mic_was_active) { dirty = true; mic_was_active = false; }
    const auto recorder_view=(!wifi_phase && !settings_open)?recorder_overlay(frame):RecorderOverlay::Hidden;
    if(recorder_view==RecorderOverlay::Settings){settings_open=true;settings_index=0;settings_dirty=true;release_gate=true;}
    if (recorder_view==RecorderOverlay::Visible) {
        both_latched=both;
        if(M5.BtnA.isPressed() || M5.BtnB.isPressed())release_gate=true;
        dirty=true;
        return;
    }
    bool suppress_buttons=release_gate;
    if (M5.BtnA.isReleased() && M5.BtnB.isReleased()) release_gate=false;
    if (both && !both_latched && !suppress_buttons && !wifi_phase) {
        settings_open=!settings_open;settings_index=0;settings_dirty=true;
        release_gate=true; suppress_buttons=true;
    }
    both_latched=both;
    if (wifi_phase) {
        if (vibrating) { M5.Power.setVibration(0); vibrating=false; }
        if (!suppress_buttons) {
            if (M5.BtnB.wasPressed()) {wifi_action=2;release_gate=true;}
            else if (M5.BtnA.wasClicked() || M5.Touch.getDetail().wasPressed()) {wifi_web_qr=!wifi_web_qr;wifi_dirty=true;}
        }
        render_wifi(); wifi_was_active=true; return;
    }
    if (wifi_was_active) {dirty=true;wifi_was_active=false;release_gate=true;suppress_buttons=true;}
    if(settings_open){
        bool choose=false;
        if(!suppress_buttons){
            if(M5.BtnA.wasClicked()){settings_index=(settings_index+1)%4;settings_dirty=true;}
            if(M5.BtnB.wasClicked())choose=true;
            const auto& touch=M5.Touch.getDetail();
            if(touch.wasPressed() && touch.x>=76 && touch.x<=390 && touch.y>=121 && touch.y<383){
                const unsigned i=(touch.y-121)/70;
                if(i<4 && (touch.y-121)%70<52){settings_index=i;choose=true;}
            }
        }
        if(choose){
            settings_open=false;dirty=true;release_gate=true;suppress_buttons=true;
            if(settings_index==0)wifi_action=1;
            else if(settings_index==1)recorder_arm(0);
            else if(settings_index==2)recorder_arm(1);
        }else{render_settings();return;}
    }
    if (state > 7) state = 0;
    if (reaction && ms - reaction_at >= 1200) { reaction = nullptr; if (!qr_mode) dirty = true; }
    if (celebrating && ms - celebration_at >= 1800) { celebrating = false; if (!qr_mode) dirty = true; }
    if (state != last_state) {
        // Celebrate an actual working/attention -> done transition, not every boot into a recent result.
        if (state == 4 && (last_state == 2 || last_state == 3) && !qr_mode && !calm) {
            celebration_at = ms;
            celebrating = true;
        }
        if (state == 3) pulse(ms);
        printf("BADGE_STATE %u\n", state);
        last_state = state;
        if (!qr_mode) dirty = true;
    }
    if (!suppress_buttons && M5.BtnA.wasClicked()) {
        if (qr_mode) qr_mode = false;
        else name_mode = !name_mode;
        dirty = true;
        printf("BADGE_VIEW %s\n", name_mode ? "name" : "pet");
    }
    if (!suppress_buttons && M5.Touch.getDetail().wasPressed()) { qr_mode = !qr_mode; dirty = true; }
    if (!suppress_buttons && M5.BtnB.wasHold() && !qr_mode) {
        calm = !calm;
        reaction = calm ? "QUIET LITTLE NICK." : "BACK TO BUSINESS.";
        reaction_at = ms;
        dirty = true;
        printf("BADGE_CALM %u\n", calm);
    }
    if (!suppress_buttons && M5.BtnB.wasClicked()) {
        static const char* replies[] = {"HEY!", "I'M SUPERVISING.", "THIS IS MY BUSY FACE.", "MORE TASKS? REALLY?"};
        qr_mode = false;
        reaction = replies[boops++ % 4];
        reaction_at = ms;
        dirty = true;
        pulse(ms);
        printf("BADGE_BOOP\n");
    }
    if (dirty) M5.Display.setBrightness(qr_mode ? 200 : (calm ? 55 : 160));
    if (qr_mode) {
        if (vibrating) { M5.Power.setVibration(0); vibrating = false; }
        if (dirty) render_qr();
        dirty = false;
        return; // QR stays still; no hidden avatar redraws or celebrations interrupt scanning.
    }
    if (calm && !dirty && state < 5) return;

    if (!calm) {
        float ax = 0, ay = 0, az = 0;
        float target = 0;
        if (M5.Imu.getAccel(&ax, &ay, &az) && std::isfinite(ax)) target = std::max(-1.0f, std::min(1.0f, ax)) * 18;
        nudge_x += (target - nudge_x) * 0.18f;
    }
    const int shift = calm ? 0 : 2 * static_cast<int>(nudge_x / 2);
    int hop = 0;
    if (!calm && reaction) {
        const auto t = ms - reaction_at;
        if (t < 700) hop = 2 * static_cast<int>(14 * std::sin(3.14159265f * t / 700));
    }
    if (!calm && celebrating) {
        const auto t = ms - celebration_at;
        if (t < 800) hop = 2 * static_cast<int>(14 * std::sin(3.14159265f * t / 800));
    }
    frame.fillScreen(TFT_BLACK);
    frame.setTextColor(CREAM, TFT_BLACK);
    frame.setFont(&fonts::FreeSansBold24pt7b);
    if (name_mode) {
        frame.drawString("NICK", 233, 81);
        frame.drawString("NISI", 233, 130);
        frame.setFont(&fonts::Font2);
        frame.setTextColor(MINT, TFT_BLACK);
        frame.drawString("@nicknisi", 233, 171);
        avatar(NICK_BLINK, 233 + shift, 190 - hop, 5, ms);
        frame.setTextColor(CREAM, TFT_BLACK);
        frame.setFont(&fonts::FreeSansBold12pt7b);
        frame.drawString("TAP FOR LINKEDIN", 233, 378);
        frame.setFont(&fonts::Font2);
        frame.setTextColor(MINT, TFT_BLACK);
        frame.drawString(reaction ? reaction : "Orange: pet / Blue: boop", 233, 415);
    } else {
        frame.drawString("RIKER", 233, 62);
        if (state == 2 || state == 3 || state == 4) {
            avatar(NICK_WAIT, 233 + shift, 104 - hop, 2, ms);
        } else if (state == 1) {
            avatar(NICK_PEEK, 233 + shift, 154 - hop, 6, ms);
        } else {
            avatar(NICK_BLINK, 233 + shift, 154 - hop, 6, ms);
        }
        if (state == 3) {
            frame.setFont(&fonts::FreeSansBold18pt7b);
            frame.setTextColor(AMBER, TFT_BLACK);
            frame.drawString("!", 333, calm ? 162 : 156 + 4 * ((ms / 250) % 2));
        }
        if (!calm && celebrating) sparkle(ms - celebration_at);
        const char* labels[] = {"RIKER IS OFFLINE", "RIKER IS IDLE", "WAITING ON RIKER", "RIKER NEEDS YOU", "MISSION COMPLETE", "SET UP WI-FI", "CONNECTING", "APPROVE IN BROWSER"};
        const char* asides[] = {"I'll be here.", "Professionally hanging out.", "Very busy waiting.", "A little help, captain?", "I helped. Emotionally."};
        frame.setFont(&fonts::FreeSansBold12pt7b);
        frame.setTextColor(state == 3 ? AMBER : MINT, TFT_BLACK);
        frame.drawString(labels[state], 233, 382);
        frame.setFont(&fonts::Font2);
        frame.setTextColor(CREAM, TFT_BLACK);
        const char* subline = reaction ? reaction : (state < 5 ? asides[state] : detail);
        frame.drawString(subline ? subline : "", 233, 408);
        frame.drawString("Tap: QR / Both: settings", 233, 434);
    }
    frame.pushSprite(0, 0);
    dirty = false;
}

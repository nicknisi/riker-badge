#include "recorder_ui.h"
#include <atomic>
#include <stdio.h>
extern "C" void badge_stream_request_stop();
extern "C" bool badge_stream_running();
extern "C" bool badge_stream_warming();
extern "C" bool badge_stream_overflowed();
extern "C" bool badge_stream_mic_active();
namespace {
uint8_t phase=0,action=0;
uint32_t captured=0,saved=0;
char message[144]={};
bool redraw=true, discard_latched=false;
std::atomic<bool> cancelled{false};
}
extern "C" bool badge_recorder_cancelled(){return cancelled.load();}
extern "C" void badge_recorder_test_prepare(){cancelled.store(false);}
extern "C" uint8_t badge_recorder_take_action(){auto a=action;action=0;return a;}
extern "C" void badge_recorder_set(uint8_t p,uint32_t c,uint32_t s,const char* msg){
    if(p!=phase)printf("RECORDER_PHASE %u\n",p);
    if(p!=phase || c!=captured || s!=saved || strcmp(message,msg))redraw=true;
    phase=p;captured=c;saved=s;snprintf(message,sizeof(message),"%s",msg);
}
void recorder_arm(uint8_t kind){phase=1;action=kind==1?2:1;redraw=true;}
RecorderOverlay recorder_overlay(M5Canvas& frame){
    const auto visibility=recorder_visibility(phase,badge_stream_mic_active(),M5.BtnA.isPressed(),M5.BtnB.isPressed());
    // Chords take precedence over individual gestures, including hold-to-discard.
    if(visibility!=RecorderOverlay::Visible)return visibility;
    if((phase==1 || phase==10) && M5.BtnA.wasPressed()){
        cancelled.store(false);action=3;phase=2;redraw=true;
    }
    if((phase==5 || phase==7) && !badge_stream_mic_active() && M5.BtnA.wasClicked()){action=6;redraw=true;}
    if(M5.BtnB.wasPressed()){
        cancelled.store(true);badge_stream_request_stop();action=4;redraw=true;
    }
    if(!M5.BtnB.isPressed())discard_latched=false;
    if(recorder_can_discard(phase,M5.BtnA.isPressed()) && !discard_latched && M5.BtnB.pressedFor(2000)){
        cancelled.store(true);badge_stream_request_stop();action=5;discard_latched=true;redraw=true;
    }
    static bool previous_running=false;
    const bool running=badge_stream_running();
    if(previous_running!=running){redraw=true;previous_running=running;}
    const bool actual_mic=badge_stream_mic_active();
    const bool warming=badge_stream_warming();
    static bool previous_mic=false,previous_warm=false;
    if(previous_mic!=actual_mic || previous_warm!=warming){redraw=true;previous_mic=actual_mic;previous_warm=warming;}
    if(!redraw)return RecorderOverlay::Visible;
    M5.Display.setBrightness(160);
    frame.fillScreen(TFT_BLACK);frame.setTextColor(TFT_WHITE,TFT_BLACK);
    frame.setFont(&fonts::FreeSansBold18pt7b);
    const char* title=phase==1?"Ready to record":phase==10?"Not started":actual_mic?(warming?"WARMING UP":"RECORDING"):phase==8?"SYNTHETIC TEST":phase==9?"Test complete":phase==6?"Saved to cloud":phase==7?"Recording stopped":"Microphone is off";
    frame.drawString(title,233,92);
    if(actual_mic)frame.fillCircle(233,141,11,TFT_RED);
    frame.setFont(&fonts::FreeSansBold24pt7b);
    char elapsed[24];snprintf(elapsed,sizeof(elapsed),"%02lu:%02lu:%02lu",static_cast<unsigned long>(captured/3600),static_cast<unsigned long>(captured/60%60),static_cast<unsigned long>(captured%60));
    frame.drawString(elapsed,233,210);
    frame.setFont(&fonts::Font2);
    char progress[72];snprintf(progress,sizeof(progress),"Saved %lus   Buffered %lus",static_cast<unsigned long>(saved),static_cast<unsigned long>(captured>saved?captured-saved:0));
    frame.drawString(phase==1?"Only record with everyone's consent.":progress,233,273);
    const char* status=warming?"Warming microphone...":(phase==4 && !running ? (badge_stream_overflowed()?"Buffer full. Capture stopped.":"Capture stopped; saving audio."):message);
    frame.drawString(status,233,320);
    frame.drawString(phase==1?"Orange: start   Blue: back":phase==10?"Orange: retry   Blue: back":(phase==6 || phase==9)?"Blue: return":"Blue: stop",233,377);
    if(phase==10)frame.drawString("Both buttons: Wi-Fi settings",233,407);
    if(phase==5 || phase==7)frame.drawString("Both: settings / hold Blue: discard",233,407);
    if(phase==6)frame.drawString("transcribe.nisi.sh",233,407);
    if(phase==8)frame.drawString("Generated data. NO microphone.",233,407);
    frame.pushSprite(0,0);redraw=false;return RecorderOverlay::Visible;
}

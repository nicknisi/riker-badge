#include "../components/badge_display/recorder_controls.h"
#include <cassert>
int main(){
 assert(recorder_visibility(10,false,true,true)==RecorderOverlay::Settings);
 assert(!recorder_can_discard(10,false));
 for(unsigned phase=0;phase<=10;++phase){
  assert(recorder_visibility(phase,false,false,false)==(phase?RecorderOverlay::Visible:RecorderOverlay::Hidden));
  assert(!recorder_can_discard(phase,true)); // Holding both must NEVER discard.
  if(phase==5 || phase==7){
   assert(recorder_visibility(phase,false,true,true)==RecorderOverlay::Settings);
   assert(recorder_visibility(phase,true,true,true)==RecorderOverlay::Visible);
   assert(recorder_visibility(phase,false,true,false)==RecorderOverlay::Visible);
   assert(recorder_visibility(phase,false,false,true)==RecorderOverlay::Visible);
   assert(recorder_can_discard(phase,false));
  }else assert(!recorder_can_discard(phase,false));
 }
}

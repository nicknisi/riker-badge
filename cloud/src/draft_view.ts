import { UNSAFE_TEXT } from "./voice.ts";
export const DRAFT_HTML = `<section id="tweet"><h2>Tweet suggestions</h2><p>Private drafts only. Nothing posts automatically. Suggestions are generated during recordings and collected below; you can also draft from a passage yourself. Automatic suggestions must be reviewed and approved before Copy or Open in X is enabled.</p><button id="useSelection" class="secondary">Use highlighted passage</button><label for="draftSource">Source material to use (optional)</label><textarea id="draftSource" rows="6" maxlength="6000" placeholder="Highlight a passage in the transcript, or paste/edit the material here."></textarea><label for="personalTake">Your take — what you actually think (optional)</label><textarea id="personalTake" rows="3" maxlength="1500" placeholder="Optional. This grounds first-person reactions instead of inventing them."></textarea><label class="check"><input id="shareable" type="checkbox"> This selected material is okay to share publicly.</label><button id="generateDrafts" disabled>Draft three options</button><p id="draftMessage" role="status"></p><label for="draftHistory">Suggestions & saved generations</label><select id="draftHistory"></select><div id="draftOptions"></div><pre id="draftSourceView"></pre><small>The starting voice profile uses Nick’s public short-form posts and writing, not his full X archive. Automatic suggestions never invent first-person reactions. Edit until it sounds like you. Verify facts, names, attribution and private information before posting. Known credential patterns are blocked; not every secret can be detected.</small></section>`;
export const DRAFT_CSS = `label{display:block;margin:18px 0 7px}textarea,select{font:inherit;box-sizing:border-box;width:100%;padding:12px;border:1px solid #647a69;border-radius:6px;background:#19251d;color:#f5f0e4}textarea{resize:vertical;line-height:1.5}textarea:focus-visible,select:focus-visible{outline:3px solid #f0bd70;outline-offset:3px}.check{display:flex;gap:10px;align-items:start;margin:18px 0}.check input{margin-top:6px}#tweet{margin:32px 0;padding:24px 0;border-top:1px solid #34463a;border-bottom:1px solid #34463a}.draft{padding:18px 0}.draft h3{margin:0 0 8px}.draft small{margin:8px 0}.draft nav{margin:10px 0}#draftSourceView:empty{display:none}#draftSourceView{white-space:pre-wrap;overflow-wrap:anywhere;background:#19251d;border:1px solid #34463a;border-radius:6px;padding:12px;margin-top:12px}#voicePanel{margin:28px 0}#voicePanel summary{cursor:pointer;font-weight:600;padding:8px 0}#voicePanel small{display:block;margin-top:12px;color:#bcc9be}`;
export const DRAFT_SCRIPT = `
let draftSeeded=false,editSession=null,draftEditors=[],draftRows=[],saveTimer,transition=false;
let saveQueue=Promise.resolve();
const seenDraftIds=new Set();
const unsafeText=new RegExp(${JSON.stringify(UNSAFE_TEXT.source)},'i');
function draftEnabled(){ $('generateDrafts').disabled=transition||!$('shareable').checked||$('draftSource').value.trim().length<20; }
function lockDrafts(value){
 transition=value;
 for(const key of ['draftSource','personalTake','shareable','draftHistory','useSelection'])$(key).disabled=value;
 for(const e of draftEditors)e.disabled=value;
 draftEnabled();
}
function fmtClock(sec){if(sec==null)return'';const m=Math.floor(sec/60);return m+':'+String(Math.floor(sec%60)).padStart(2,'0');}
function sourceChanged(){draftSeeded=true;$('shareable').checked=false;draftEnabled();}
$('shareable').onchange=draftEnabled;$('draftSource').oninput=sourceChanged;$('personalTake').oninput=sourceChanged;
$('useSelection').onmousedown=e=>e.preventDefault();
$('useSelection').onclick=()=>{
 const s=window.getSelection();if(!s||!s.toString().trim())return;
 if(!$('notes').contains(s.anchorNode)&&!$('transcript').contains(s.anchorNode)){text('draftMessage','Select a passage from the notes or transcript.');return;}
 $('draftSource').value=s.toString().slice(0,6000);sourceChanged();$('tweet').scrollIntoView({behavior:'smooth'});
};
function saveEdits(){
 if(!editSession)return Promise.resolve();
 const session=editSession,texts=draftEditors.map(e=>e.value),serialized=JSON.stringify(texts);
 // Capture identity AND values now; serialize requests, then CAS protects other tabs.
 const save=async()=>{
  if(session.saved===serialized)return;
  if(texts.some(t=>unsafeText.test(t)))throw Error('Remove possible credentials or unsafe characters. Your text is still here.');
  const result=await api('/api/recordings/'+id+'/drafts/'+session.id,{method:'PATCH',headers:{'content-type':'application/json'},body:JSON.stringify({texts,revision:session.revision})});
  session.revision=result.revision;session.saved=serialized;
  const row=draftRows.find(r=>r.id===session.id);
  if(row){row.payload=JSON.stringify(JSON.parse(row.payload).map((d,i)=>({...d,text:texts[i]})));row.revision=result.revision;}
  if(editSession===session)text('draftMessage','Edits saved privately. Nothing posted.');
 };
 saveQueue=saveQueue.catch(()=>{}).then(save);return saveQueue;
}
async function approveCurrent(){
 if(!editSession||transition)return;
 const session=editSession,texts=draftEditors.map(e=>e.value);
 if(texts.some(t=>unsafeText.test(t))){text('draftMessage','Remove possible credentials or unsafe characters before approving.');return;}
 lockDrafts(true);clearTimeout(saveTimer);
 try{
  const result=await api('/api/recordings/'+id+'/drafts/'+session.id,{method:'PATCH',headers:{'content-type':'application/json'},body:JSON.stringify({texts,revision:session.revision,approve:true})});
  session.revision=result.revision;session.saved=JSON.stringify(texts);session.approved=true;
  const row=draftRows.find(r=>r.id===session.id);
  if(row){row.approved=1;row.revision=result.revision;row.payload=JSON.stringify(JSON.parse(row.payload).map((d,i)=>({...d,text:texts[i]})));}
  if(editSession===session&&row)showDrafts(row);
  text('draftMessage','Approved privately. You can copy or Open in X now. Nothing posts automatically.');
 }catch(e){text('draftMessage','Not approved: '+e.message);}
 finally{lockDrafts(false);}
}
async function viewSource(){
 if(!editSession)return;
 try{const s=await api('/api/recordings/'+id+'/drafts/'+editSession.id);$('draftSourceView').textContent=s.source_text?('Source '+(s.window_start!=null?fmtClock(s.window_start)+'–'+fmtClock(s.window_end)+': ':'')+s.source_text):'No source snapshot was kept.';}
 catch(e){text('draftMessage','Could not load source: '+e.message);}
}
function renderHistory(){
 $('draftHistory').replaceChildren();
 for(const row of draftRows){
  const option=document.createElement('option');option.value=row.id;option.disabled=!row.payload;
  const range=row.window_start!=null?' · '+fmtClock(row.window_start)+'–'+fmtClock(row.window_end):'';
  const state=row.error?' — failed':!row.payload?' — processing':row.kind==='auto'&&!row.approved?' — needs review':'';
  option.textContent=(row.kind==='auto'?'Auto ':'Manual ')+new Date(row.created_at).toLocaleString()+range+state;
  $('draftHistory').append(option);
 }
 if(editSession)$('draftHistory').value=editSession.id;
}
function showDrafts(row){
 if(!row.payload)return;
 const values=JSON.parse(row.payload);
 editSession={id:row.id,revision:row.revision,saved:JSON.stringify(values.map(d=>d.text)),kind:row.kind,approved:!!row.approved};
 const needsApproval=editSession.kind==='auto'&&!editSession.approved;
 $('draftSourceView').textContent='';
 $('draftOptions').replaceChildren();draftEditors=[];
 for(const d of values){
  const article=document.createElement('article');article.className='draft';
  const title=document.createElement('h3');title.textContent=d.angle;
  const area=document.createElement('textarea');area.rows=4;area.maxLength=2000;area.value=d.text;area.setAttribute('aria-label',d.angle+' draft');draftEditors.push(area);
  const count=document.createElement('small'),nav=document.createElement('nav'),copy=document.createElement('button'),open=document.createElement('button');
  copy.textContent='Copy';open.textContent='Open in X';
  const update=()=>{
   const unsafe=unsafeText.test(area.value);
   count.textContent=unsafe?'Possible credential or unsafe characters — remove before exporting.':needsApproval?Array.from(area.value).length+' characters — review and approve to enable Copy / Open in X.':Array.from(area.value).length+' characters — X shows the final count for links and emoji.';
   copy.disabled=open.disabled=unsafe||needsApproval;
  };update();
  area.oninput=()=>{update();clearTimeout(saveTimer);saveTimer=setTimeout(()=>saveEdits().catch(e=>text('draftMessage','Not saved: '+e.message)),900);};
  copy.onclick=()=>{
   if(unsafeText.test(area.value)||needsApproval)return;
   navigator.clipboard.writeText(area.value).then(()=>text('draftMessage','Copied. Nothing posted.')).catch(()=>text('draftMessage','Select and copy the text manually.'));
   void saveEdits().catch(e=>text('draftMessage','Copy requested, but edits were not saved: '+e.message));
  };
  open.onclick=async()=>{
   if(unsafeText.test(area.value)||needsApproval||transition)return;
   const intent='https://twitter.com/intent/tweet?text='+encodeURIComponent(area.value);
   lockDrafts(true);clearTimeout(saveTimer);
   try{await saveEdits();window.location.assign(intent);}catch(e){text('draftMessage','Not opened: '+e.message);lockDrafts(false);}
  };
  nav.append(copy,open);
  if(editSession.kind==='auto'){
   const approve=document.createElement('button');approve.className='secondary';approve.textContent=editSession.approved?'Approved':'Approve for export';approve.disabled=editSession.approved;approve.onclick=approveCurrent;
   const src=document.createElement('button');src.className='secondary';src.textContent='View source';src.onclick=viewSource;
   nav.append(approve,src);
  }
  article.append(title,area,count,nav);$('draftOptions').append(article);
 }
 renderHistory();
}
async function loadDrafts(row){
 if(!draftSeeded&&row.notes){$('draftSource').value=row.notes.slice(0,6000);draftSeeded=true;draftEnabled();}
 const result=await api('/api/recordings/'+id+'/drafts');
 if(transition)return;
 const current=draftRows.find(r=>r.id===editSession?.id);
 draftRows=result.generations.map(r=>current&&r.id===current.id&&r.revision<editSession.revision?current:r);
 // A poll never replaces an active editor, even if it falls outside the newest rows.
 if(current&&!draftRows.some(r=>r.id===current.id))draftRows.push(current);
 const fresh=draftRows.filter(r=>r.payload&&!seenDraftIds.has(r.id));
 for(const r of draftRows)seenDraftIds.add(r.id);
 renderHistory();
 if(!editSession){const ready=draftRows.find(r=>r.payload);if(ready)showDrafts(ready);}
 else{const arrived=fresh.filter(r=>r.kind==='auto').length;if(arrived)text('draftMessage',arrived+' new private suggestion'+(arrived>1?'s':'')+' arrived — see history below. Your edits are kept.');}
}
$('draftHistory').onchange=async()=>{
 const target=$('draftHistory').value;clearTimeout(saveTimer);lockDrafts(true);
 try{await saveEdits();const row=draftRows.find(r=>r.id===target);if(row)showDrafts(row);}
 catch(e){text('draftMessage','Kept your current edits: '+e.message);renderHistory();}
 finally{lockDrafts(false);}
};
$('generateDrafts').onclick=async()=>{
 const input={source:$('draftSource').value,take:$('personalTake').value,shareable:$('shareable').checked};
 clearTimeout(saveTimer);lockDrafts(true);
 try{
  await saveEdits();text('draftMessage','Drafting from the selected material only…');
  const result=await api('/api/recordings/'+id+'/drafts',{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify(input)});
  const row={id:result.id,revision:result.revision,created_at:result.created_at,kind:'manual',approved:1,payload:JSON.stringify(result.drafts),error:null};
  seenDraftIds.add(row.id);draftRows.unshift(row);showDrafts(row);text('draftMessage','Three private drafts. Check the facts, edit, then copy or open X to post yourself.');
 }catch(e){text('draftMessage',e.message+' Nothing was posted.');}
 finally{lockDrafts(false);}
};
`;
export const VOICE_SETTINGS_HTML = `<details id="voicePanel"><summary>Voice &amp; automatic suggestions</summary><p>Private, owner-only settings used to tailor drafts to your voice. Example posts are a tone reference only — never treated as facts or as your opinions.</p><label class="check"><input id="autoEnabled" type="checkbox"> Generate private tweet suggestions automatically during recordings</label><label for="cadence">How often to consider a new transcript window</label><select id="cadence"></select><label for="styleInstructions">Style instructions (leave blank to use the built-in profile)</label><textarea id="styleInstructions" rows="4" maxlength="4000" placeholder="Describe how your posts should sound."></textarea><label for="examplePosts">Example posts (tone reference only, not facts)</label><textarea id="examplePosts" rows="5" maxlength="6000" placeholder="Paste a few of your own posts to mimic rhythm and voice."></textarea><button id="saveVoice" class="secondary">Save voice settings</button><p id="voiceMessage" role="status"></p><small id="voiceFallback"></small></details>`;
export const VOICE_SETTINGS_SCRIPT = `
let voiceRevision=0,voiceLoaded=false;
async function loadVoice(){
 if(voiceLoaded)return;
 try{
  const v=await api('/api/voice');
  $('autoEnabled').checked=v.auto_enabled;
  $('cadence').replaceChildren();
  for(const c of v.cadence_choices){const o=document.createElement('option');o.value=c;o.textContent='Every '+c+' min';if(c===v.cadence_minutes)o.selected=true;$('cadence').append(o);}
  $('styleInstructions').value=v.style_instructions;$('examplePosts').value=v.example_posts;voiceRevision=v.revision;voiceLoaded=true;
  text('voiceFallback','Built-in fallback voice profile: '+v.fallback);
 }catch(e){text('voiceMessage',e.message);}
}
$('saveVoice').onclick=async()=>{
 try{
  $('saveVoice').disabled=true;
  const body={style_instructions:$('styleInstructions').value,example_posts:$('examplePosts').value,auto_enabled:$('autoEnabled').checked,cadence_minutes:Number($('cadence').value),revision:voiceRevision};
  const r=await api('/api/voice',{method:'PUT',headers:{'content-type':'application/json'},body:JSON.stringify(body)});
  voiceRevision=r.revision;text('voiceMessage','Saved privately. New windows use these settings.');
 }catch(e){text('voiceMessage','Not saved: '+e.message);}
 finally{$('saveVoice').disabled=false;}
};
`;

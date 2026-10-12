import {
  DRAFT_HTML,
  DRAFT_CSS,
  DRAFT_SCRIPT,
  VOICE_SETTINGS_HTML,
  VOICE_SETTINGS_SCRIPT,
} from "./draft_view.ts";
// A small owner-only reader. All meeting/model text is assigned via textContent, never HTML.
export const PAGE = `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Transcribe</title><style>
:root{color-scheme:dark}body{font:17px/1.55 system-ui;background:#101512;color:#f5f0e4;margin:0}main{max-width:850px;margin:auto;padding:40px 24px}h1{font-size:42px;letter-spacing:-.025em;margin:0}h2{margin-top:36px}a{color:#b2e8be;text-underline-offset:4px}p.meta,small{color:#bcc9be}button,a.button{font:inherit;background:#b2e8be;color:#142219;border:0;border-radius:6px;padding:10px 14px;cursor:pointer;text-decoration:none}button.secondary{background:#29372e;color:#f5f0e4}button:disabled{opacity:.55;cursor:wait}button:focus-visible,a:focus-visible{outline:3px solid #f0bd70;outline-offset:4px}ul{padding:0;list-style:none}li{padding:20px 0;border-bottom:1px solid #34463a}li a{font-size:21px}pre{white-space:pre-wrap;overflow-wrap:anywhere;font:inherit;margin:0}nav{display:flex;gap:12px;flex-wrap:wrap;margin:20px 0}#error{color:#ffc39f}#detail[hidden],#list[hidden]{display:none}audio{width:100%}.clips{display:flex;gap:10px;flex-wrap:wrap}.clips button{font-size:14px;padding:7px 10px}small{display:block;margin-top:8px}@media(max-width:500px){main{padding:26px 18px}h1{font-size:34px}}
${DRAFT_CSS}</style><script src="/app.js" defer></script></head><body><main><h1>Transcribe</h1><p class="meta">Private meeting notes from your badge.</p><p id="error" role="alert"></p><section id="list"><p id="loading" role="status">Loading recordings…</p><ul id="recordings"></ul>${VOICE_SETTINGS_HTML}<small>Start recording deliberately on the badge, with everyone’s consent. Audio access expires after 7 days; transcripts and notes stay until you delete them. Up to 8 hours per recording and per rolling day.</small></section><section id="detail" hidden><a href="/">← All recordings</a><h2 id="title"></h2><p class="meta" id="status" role="status"></p><nav><a id="download" class="button">Download Markdown</a><button id="retry" class="secondary">Retry processing</button><button id="partial" class="secondary">Close as interrupted</button><button id="delete" class="secondary">Delete recording</button></nav><p id="warning"></p><h2>Notes</h2><pre id="notes"></pre><small>AI-generated draft. Check facts and commitments against the transcript; no action items are executed.</small>${DRAFT_HTML}<h2>Transcript</h2><pre id="transcript"></pre><h2>Replay a chunk</h2><audio id="player" controls preload="none"></audio><div class="clips" id="clips"></div></section></main></body></html>`;

export const SCRIPT = `
const $=id=>document.getElementById(id);
const id=new URL(location.href).searchParams.get('id');
const valid=/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
async function api(path,options){const r=await fetch(path,options);const j=await r.json();if(!r.ok)throw Error(j.error||'Request failed');return j;}
function text(id,value){if($(id).textContent!==(value||''))$(id).textContent=value||'';}
async function load(){
 try{
  text('error','');
  if(!id){
   const rows=await api('/api/recordings');$('recordings').replaceChildren();
   text('loading',rows.length?'':'No recordings yet. Your microphone is never started by this page.');
   for(const row of rows){const li=document.createElement('li'),a=document.createElement('a'),small=document.createElement('small');a.href='/?id='+encodeURIComponent(row.id);a.textContent=row.title;small.textContent=new Date(row.created_at).toLocaleString()+' · '+row.status;li.append(a,small);$('recordings').append(li);}
   await loadVoice();return;
  }
  if(!valid.test(id))throw Error('Invalid recording ID');
  $('list').hidden=true;$('detail').hidden=false;
  const row=await api('/api/recordings/'+id);
  text('title',row.title);text('status',row.status+' · '+(row.total_samples===null?'Still open':(row.total_samples/16000/60).toFixed(1)+' minutes'));
  text('notes',row.notes||'Notes are not ready yet. Saved audio and transcription progress are tracked separately.');if(!window.getSelection()?.toString())text('transcript',row.transcript);
  await loadDrafts(row);
  text('warning',row.gaps.length?row.gaps.length+' intervals have no usable transcript yet. Do not treat this as a complete record.':'');
  $('download').href='/api/recordings/'+id+'/notes';$('partial').hidden=row.expected_chunks!==null;
  $('retry').hidden=!['partial','notes-failed'].includes(row.status);
  $('clips').replaceChildren();
  if(Date.now()>=row.expires_at) text('clips','Original audio has expired. Storage cleanup may complete later; it is no longer accessible here.');
  else for(const c of row.chunks){if(c.status==='receiving')continue;const b=document.createElement('button');b.className='secondary';b.textContent=Math.floor(c.seq/6)+':'+String(c.seq%6*10).padStart(2,'0');b.onclick=()=>{$('player').src='/api/recordings/'+id+'/audio/'+c.seq;$('player').play().catch(()=>text('error','Could not play this audio chunk.'));};$('clips').append(b);}
 }catch(e){text('error',e.message+' Reload to sign in again if your session expired.');}
}
for(const [button,action] of [['retry','retry'],['partial','close-partial']])$(button).onclick=async()=>{if(!confirm(action==='retry'?'Retry failed processing? This may use additional AI credits.':'Close this as an incomplete recording?'))return;try{$(button).disabled=true;await api('/api/recordings/'+id+'/'+action,{method:'POST',headers:{'content-type':'application/json'},body:'{}'});await load();}catch(e){text('error',e.message);}finally{$(button).disabled=false;}};
$('delete').onclick=async()=>{if(!confirm('Delete this recording, its audio, transcript and notes? This cannot be undone.'))return;try{await api('/api/recordings/'+id,{method:'DELETE'});location.href='/';}catch(e){text('error',e.message);}};
${DRAFT_SCRIPT}
${VOICE_SETTINGS_SCRIPT}
load();setInterval(()=>{if(!document.hidden)load();},15000);
`;

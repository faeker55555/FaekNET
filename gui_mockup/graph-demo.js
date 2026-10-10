/* Interactive reference only. Every RTT, probe, commit and packet here is simulated. */
'use strict';
const $ = s => document.querySelector(s);
const nodes = [
 {id:'you',name:'faeker',ip:'10.66.0.1',online:true,relay:true},
 {id:'alice',name:'alice',ip:'10.66.0.2',online:true,relay:true},
 {id:'gaming-pc',name:'gaming-pc',ip:'10.66.0.3',online:true,relay:true},
 {id:'laptop',name:'laptop',ip:'10.66.0.4',online:false,relay:true},
 {id:'nas',name:'nas',ip:'10.66.0.5',online:true,relay:true},
 {id:'studio',name:'studio',ip:'10.66.0.6',online:true,relay:true},
 {id:'max',name:'max',ip:'10.66.0.7',online:true,relay:true},
 {id:'server',name:'server',ip:'10.66.0.8',online:true,relay:true},
 {id:'phone',name:'phone',ip:'10.66.0.9',online:true,relay:true}
];
const links = [
 ['you','alice',64],['you','gaming-pc',9],['gaming-pc','alice',14],['you','nas',80],
 ['gaming-pc','nas',19],['alice','nas',22],['you','studio',42],['studio','max',26],
 ['you','max',114],['you','server',35],['server','phone',18],['studio','gaming-pc',28],
 ['server','gaming-pc',41],['laptop','alice',null]
].map(([a,b,base])=>({a,b,base,rtt:base||90,up:base!==null}));
let selected='alice',running=true,toastTimer,failedRelay=null,syncing=false,sequence=0,probeTimer;
const model = new MeshModel.Routes(nodes);
const manifest = new Map(nodes.map((n,i)=>[n.id,{endpoint:`203.0.113.${24+i}:${54321+i}`,activeEndpoint:`203.0.113.${24+i}:${54321+i}`,revision:'9c4e1a7',state:'Installed',candidate:null}]));
const events=[];
function logEvent(text){events.unshift(text);events.splice(8);$('#event-log').replaceChildren(...events.map(text=>{const row=document.createElement('div');row.textContent=text;return row}));}
function notify(text){clearTimeout(toastTimer);$('.toast').textContent=text;$('.toast').hidden=false;toastTimer=setTimeout(()=>$('.toast').hidden=true,4000)}
function edge(a,b){return links.find(l=>(l.a===a&&l.b===b)||(l.b===a&&l.a===b))}
function candidates(target){const result=[];const direct=edge('you',target);if(direct?.up)result.push({path:['you',target],rtt:direct.rtt});for(const n of nodes){if(n.id==='you'||n.id===target||!n.online||!n.relay)continue;const a=edge('you',n.id),b=edge(n.id,target);if(a?.up&&b?.up)result.push({path:['you',n.id,target],rtt:a.rtt+b.rtt+1});}return result;}
function sample(now=performance.now()){
 if(!running)return;
 for(const l of links)if(l.up)l.rtt=l.base+Math.sin(now/5500+links.indexOf(l))*.6;
 for(const n of nodes){if(n.id==='you'||n.probing)continue;for(const c of candidates(n.id))model.observe(c.path,c.rtt,now,true);}
 render();
}
const graph = new PingGraph($('#force-map'),nodes,links,id=>select(id));
function select(id){selected=id;graph.selected=id;render();}
function routeFor(id){return id==='you'?{path:['you'],rtt:0,samples:3,via:null}:model.choose(id,performance.now());}
function renderPeers(){
 const q=$('#peer-search').value.trim().toLowerCase();const peers=nodes.filter(n=>n.id!=='you'&&(n.name+' '+n.ip).includes(q));$('#peer-list').replaceChildren();
 for(const p of peers){const route=routeFor(p.id);const row=document.createElement('button');row.className='peer-row'+(p.id===selected?' selected':'');row.setAttribute('aria-pressed',String(p.id===selected));row.innerHTML=`<span class="peer-orb ${p.online&&running?'':'offline'}"></span><span><span class="peer-name">${p.name}</span><div class="peer-ip">${p.ip}</div></span><span class="peer-state">${!running?'Paused':route?Math.round(route.rtt)+' ms':p.probing?'Probing':p.online?'No route':'Offline'}</span>`;row.onclick=()=>select(p.id);$('#peer-list').append(row);}
 if(!peers.length){const e=document.createElement('div');e.className='empty';e.textContent='No matching peers';$('#peer-list').append(e);}
}
function render(){
 const p=nodes.find(n=>n.id===selected),route=running?routeFor(selected):null;
 graph.route=route?.path||[];graph.draw();renderPeers();
 $('#detail-name').textContent=p.name;$('#detail-ip').textContent=p.ip;$('#detail-domain').textContent=p.name+'.mesh';
 $('#detail-discovery').textContent='Sample manifest';$('#detail-latency').textContent=route?Math.round(route.rtt)+' ms':'—';
 $('#detail-status').textContent=!running?'Paused':p.probing?'Probing':route?'Reachable':p.online?'No route':'Offline';
 $('#detail-status').className='pill'+(!route?' offline':'');
 $('#relay-consent').checked=p.relay;$('#relay-consent').disabled=syncing;
 $('#detail-endpoint').textContent=manifest.get(p.id).activeEndpoint;
 const direct=edge('you',selected);$('#rtt-slider').disabled=!direct||!running;$('#rtt-slider').value=direct?.base||0;$('#rtt-readout').textContent=direct?Math.round(direct.rtt)+' ms':'No direct link';
 $('#route-kind').textContent=selected==='you'?'Local device':route?.via?'Peer relay':route?'Direct path':'No validated route';
 $('#route-ms').textContent=route?Math.round(route.rtt)+' ms':'—';
 const path=route?.path||[];$('#route-path').replaceChildren();for(let i=0;i<path.length;i++){if(i){const arrow=document.createElement('span');arrow.className='path-arrow';arrow.textContent='→';$('#route-path').append(arrow)}const item=document.createElement('span');item.className='path-node';item.textContent=nodes.find(n=>n.id===path[i]).name;$('#route-path').append(item);}
 if(!path.length)$('#route-path').textContent='Waiting for three successful sample probes.';
 $('#route-evidence').textContent=route?.via?'3+ valid sample probes · one relay hop · 20% / 3 ms switch margin':route?'Validated sample path · keep current route until a better path qualifies':'Unavailable routes are not used for forwarding.';
 $('#route-comparison').textContent=route?.via&&direct?`Direct: ${Math.round(direct.rtt)} ms`:'Latency ≠ throughput';
 $('#online-count').textContent=running?String(nodes.filter(n=>n.id!=='you'&&n.online).length):'0';$('#peer-caption').textContent=running?'Sample mesh':'Mesh paused';$('#active-paths').textContent=(running?nodes.filter(n=>n.id!=='you'&&model.choose(n.id,performance.now())).length:0)+' reachable peers';
 $('#nat-change').disabled=syncing||!running;$('#fail-relay').disabled=syncing||!running;$('#sync-manifest').disabled=syncing;
 $('#fail-relay').textContent=failedRelay?'Restore relay':'Fail active relay';
}
function renderManifest(){
 $('#manifest-rows').replaceChildren();for(const n of nodes){const record=manifest.get(n.id);const tr=document.createElement('tr');tr.dataset.peer=n.id;tr.classList.toggle('changed',Boolean(record.candidate));for(const [text,cls] of [[n.name,''],[n.ip,'mono'],[record.endpoint,'mono'],[record.revision,'mono revision'],[record.state,'manifest-state']]){const td=document.createElement('td');td.textContent=text;td.className=cls;tr.append(td)}$('#manifest-rows').append(tr);}
}
const delay=ms=>new Promise(resolve=>setTimeout(resolve,ms));
async function simulateNat(id=selected,reject=false){
 if(syncing||!running)return false;
 syncing=true;const p=nodes.find(n=>n.id===id),r=manifest.get(id);const old=r.activeEndpoint;const next=`198.51.100.${40+nodes.indexOf(p)}:${56000+(++sequence)}`;
 r.candidate=next;r.state='Detected';renderManifest();render();logEvent(`${p.name}: observed ${next}. Active endpoint unchanged.`);
 await delay(450);r.state='Queued';renderManifest();logEvent('Demo request queued for enrolled GitHub identity. No key in payload.');
 await delay(450);r.revision=(0x9c4e1a7+sequence).toString(16);r.endpoint=next;r.state='Committed';$('#manifest-revision').textContent=r.revision;renderManifest();logEvent(`Demo revision ${r.revision}: same virtual IP, new endpoint.`);
 await delay(450);r.state='Probing';p.probing=true;renderManifest();render();logEvent('Pulled sample manifest. Challenge new endpoint before promotion.');
 // Actual client must keep identity, verify authenticated replies, and only
 // promote after validation. This delay merely illustrates that contract.
 await delay(1000);
 if(reject){r.state='Rejected probe';r.candidate=null;p.probing=false;logEvent('Probe rejected. Old endpoint retained; no blind address switch.');notify('Simulated probe failure: active endpoint was not changed.');}
 else{r.activeEndpoint=next;r.candidate=null;r.state='Installed';p.probing=false;model.invalidate(id);const now=performance.now();for(let i=2;i>=0;i--)for(const c of candidates(id))model.observe(c.path,c.rtt,now-i*10,true);logEvent(`${p.name}: three valid sample replies. Candidate promoted.`);notify('Simulated NAT update installed. Nothing was written to GitHub.');}
 syncing=false;renderManifest();render();return !reject&&r.activeEndpoint!==old;
}
$('#peer-search').addEventListener('input',renderPeers);
$('#reset-map').onclick=()=>{graph.reset();$('#peer-search').value='';select('alice')};
$('#zoom-in').onclick=()=>graph.zoom(1.15);$('#zoom-out').onclick=()=>graph.zoom(1/1.15);
$('#motion-toggle').onclick=()=>{graph.motion=!graph.motion;$('#motion-toggle').setAttribute('aria-pressed',String(graph.motion));$('#motion-toggle').textContent=graph.motion?'Pause motion':'Resume motion'};
$('#relay-consent').onchange=e=>{nodes.find(n=>n.id===selected).relay=e.target.checked;render();logEvent(`${selected}: relay ${e.target.checked?'enabled':'disabled'} in simulation.`)};
$('#rtt-slider').oninput=e=>{const link=edge('you',selected);if(link){link.base=Number(e.target.value);link.rtt=link.base;render()}};
$('#nat-change').onclick=()=>simulateNat();
$('#fail-relay').onclick=()=>{if(failedRelay){nodes.find(n=>n.id===failedRelay).online=true;logEvent(`${failedRelay}: restored; validating sample paths.`);failedRelay=null}else{const route=routeFor(selected);const id=route?.via||'gaming-pc';nodes.find(n=>n.id===id).online=false;model.invalidate(id);failedRelay=id;logEvent(`${id}: unavailable. Select a validated fallback, not a blind reconnect.`)}render()};
$('#sync-manifest').onclick=()=>{renderManifest();notify('Sample manifest checked. No GitHub request was made.');logEvent('Checked sample repository revision '+$('#manifest-revision').textContent)};
$('#toggle-mesh').onclick=()=>{if(syncing)return notify('Wait for the current sample transaction to finish.');running=!running;document.body.classList.toggle('paused',!running);$('#status-value').textContent=running?'Connected':'Paused';$('#status-caption').textContent=running?'Routing simulation':'Simulation paused';$('#toggle-mesh span').textContent=running?'Pause':'Resume';$('#toggle-mesh use').setAttribute('href',running?'#pause':'#play');if(!running){for(const n of nodes)model.invalidate(n.id)}else{const now=performance.now();for(let i=2;i>=0;i--)sample(now-i*10)}render()};
$('#theme-toggle').onclick=()=>{const dark=document.documentElement.dataset.theme!=='dark';document.documentElement.dataset.theme=dark?'dark':'light';$('#theme-toggle span').textContent=dark?'Light appearance':'Dark appearance';$('#theme-toggle').setAttribute('aria-label',dark?'Switch to light theme':'Switch to dark theme')};
$('#copy-address').onclick=async()=>{const ip=nodes.find(n=>n.id===selected).ip;try{await navigator.clipboard.writeText(ip);notify('Copied '+ip)}catch{notify('Clipboard unavailable. Select and copy: '+ip)}};
const dialogs={add:{title:'Add peer',html:'<p>Enroll a GitHub identity and virtual IP in the native directory. This sample graph does not enroll real users.</p>'},directory:{title:'Repository sync',html:'<p>Think of the peer directory as a manifest: identity stays fixed; endpoint revisions change.</p><ol><li>Client detects a fresh public IP:port.</li><li>Enrolled user submits metadata; GitHub Actions commits it.</li><li>Peers pull the new manifest and probe the candidate.</li><li>Only a valid authenticated reply promotes the endpoint.</li></ol><p>Git is eventually consistent, not real-time rendezvous. It cannot make an unreachable NAT mapping reachable. Native peer relay transport is not implemented yet.</p>'},domains:{title:'Domains',html:'<p>Sample peer names resolve conceptually to their stable 10.66.0.x addresses. This reference does not modify DNS.</p>'},activity:{title:'Activity',html:'<p>The event list below the repository shows simulated route and endpoint transitions only. No real packets, commits, or GitHub requests are generated.</p>'},settings:{title:'Routing safeguards',html:'<p>Simulation rules: at most one relay, explicit relay permission, three fresh valid sample probes, a 20% / 3 ms improvement margin, and a 10-second hold-down. Loss of a route bypasses the hold-down only for an already validated fallback.</p><p>Real forwarding needs an authenticated relay protocol, bounded queues, loop prevention, per-peer authorization and multi-machine tests. The current shared-key mesh is not a trustless relay network.</p>'}};
document.querySelectorAll('[data-dialog]').forEach(b=>b.onclick=()=>{const d=dialogs[b.dataset.dialog];$('#modal-title').textContent=d.title;$('#modal-content').innerHTML=d.html;$('#modal').showModal()});$('#close-dialog').onclick=$('#done-dialog').onclick=()=>$('#modal').close();
$('#overview-nav').onclick=()=>window.scrollTo({top:0,behavior:'smooth'});$('#peers-nav').onclick=()=>{$('#peers-panel').scrollIntoView({behavior:'smooth',block:'center'});$('#peer-search').focus({preventScroll:true})};
const now=performance.now();for(let i=2;i>=0;i--)sample(now-i*10);renderManifest();logEvent('Sample manifest loaded. Select a node to inspect its route.');probeTimer=setInterval(sample,1000);
window.meshDemo={nodes,links,model,graph,select,simulateNat,manifest,get selected(){return selected},get syncing(){return syncing},stopSamples(){clearInterval(probeTimer)},sample};

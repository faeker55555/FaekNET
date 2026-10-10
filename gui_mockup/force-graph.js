/* Dependency-free, ping-weighted force graph for sample data. */
class PingGraph {
  constructor(host, nodes, links, onSelect) {
    this.host = host; this.nodes = nodes; this.links = links; this.onSelect = onSelect;
    this.selected = 'alice'; this.route = []; this.motion = true; this.camera = {x:0,y:0,k:1};
    const ns = 'http://www.w3.org/2000/svg';
    const el = (tag, attrs={}) => { const e=document.createElementNS(ns,tag);for(const [k,v] of Object.entries(attrs))e.setAttribute(k,v);return e; };
    this.svg=el('svg',{viewBox:'0 0 900 500',role:'group','aria-label':'Draggable sample peer graph; edge labels show simulated RTT'});
    this.world=el('g');this.svg.append(this.world);host.replaceChildren(this.svg);
    this.linkViews=links.map(l=>{const line=el('line');const label=el('text',{'text-anchor':'middle',class:'edge-label'});this.world.append(line,label);return {l,line,label};});
    const initial={you:[365,290],alice:[540,210],'gaming-pc':[410,170],laptop:[765,405],nas:[600,115],studio:[260,110],max:[100,190],server:[330,410],phone:[145,410]};
    this.nodeViews=nodes.map(n=>{[n.x,n.y]=initial[n.id];n.renderX=n.x;n.renderY=n.y;n.vx=0;n.vy=0;const g=el('g',{tabindex:'0',role:'button','data-node':n.id,'aria-label':`Select ${n.name}`});const halo=el('circle',{r:14,class:'node-halo'});const dot=el('circle',{r:n.id==='you'?8:5.5,class:'graph-dot'});const text=el('text',{y:24,'text-anchor':'middle',class:'graph-label'});text.textContent=n.name+(n.id==='you'?' · you':'');g.append(halo,dot,text);this.world.append(g);g.addEventListener('keydown',e=>{if(e.key==='Enter'||e.key===' '){e.preventDefault();onSelect(n.id)}});return {n,g,halo,dot};});
    this.svg.addEventListener('pointerdown',e=>{if(e.button!==0)return;const node=e.target.closest('[data-node]');const n=node&&nodes.find(n=>n.id===node.dataset.node);this.drag={id:e.pointerId,node:n,start:this.point(e),last:this.point(e),moved:false};this.svg.setPointerCapture(e.pointerId);if(n)n.fixed=true;});
    this.svg.addEventListener('pointermove',e=>{if(!this.drag||e.pointerId!==this.drag.id)return;const p=this.point(e),d=this.drag;const dx=p.x-d.last.x,dy=p.y-d.last.y;if(Math.hypot(p.x-d.start.x,p.y-d.start.y)>3)d.moved=true;if(d.node){d.node.x+=dx/this.camera.k;d.node.y+=dy/this.camera.k;d.node.vx=d.node.vy=0}else{this.camera.x+=dx;this.camera.y+=dy}d.last=p;this.draw();});
    const end=e=>{const d=this.drag;if(!d||e.pointerId!==d.id)return;if(d.node){d.node.fixed=false;if(!d.moved)this.onSelect(d.node.id)}this.drag=null;};
    this.svg.addEventListener('pointerup',end);this.svg.addEventListener('pointercancel',end);this.svg.addEventListener('lostpointercapture',end);
    this.svg.addEventListener('wheel',e=>{e.preventDefault();const p=this.point(e);this.zoom(e.deltaY<0?1.1:1/1.1,p);},{passive:false});
    this.reduced=matchMedia('(prefers-reduced-motion: reduce)');
    // Settle once before first paint, so the static screenshot is meaningful.
    for(let i=0;i<180;i++)this.step();
    this.frame=()=>{if(this.motion&&!this.reduced.matches&&!document.hidden)this.step();this.draw();this.raf=requestAnimationFrame(this.frame);};this.frame();
  }
  point(e){const p=this.svg.createSVGPoint();p.x=e.clientX;p.y=e.clientY;return p.matrixTransform(this.svg.getScreenCTM().inverse())}
  zoom(factor,p={x:450,y:250}){const old=this.camera.k,k=Math.max(.55,Math.min(2.5,old*factor));this.camera.x=p.x-(p.x-this.camera.x)*k/old;this.camera.y=p.y-(p.y-this.camera.y)*k/old;this.camera.k=k;this.draw()}
  reset(){this.camera={x:0,y:0,k:1};this.draw()}
  step(){
    for(let i=0;i<this.nodes.length;i++){const a=this.nodes[i];for(let j=i+1;j<this.nodes.length;j++){const b=this.nodes[j];let dx=b.x-a.x,dy=b.y-a.y;const dist=Math.max(8,Math.hypot(dx,dy)),force=Math.min(2,2400/(dist*dist));dx=dx/dist*force;dy=dy/dist*force;a.vx-=dx;a.vy-=dy;b.vx+=dx;b.vy+=dy;}}
    for(const l of this.links){if(!l.up)continue;const a=this.nodes.find(n=>n.id===l.a),b=this.nodes.find(n=>n.id===l.b);if(!a.online||!b.online)continue;const dx=b.x-a.x,dy=b.y-a.y,dist=Math.max(1,Math.hypot(dx,dy));const target=65+Math.min(110,l.rtt)*2.1;const force=(dist-target)*.006;a.vx+=dx/dist*force;a.vy+=dy/dist*force;b.vx-=dx/dist*force;b.vy-=dy/dist*force;}
    for(const n of this.nodes){if(n.fixed)continue;n.vx+=(450-n.x)*.00035;n.vy+=(250-n.y)*.00035;n.vx*=.87;n.vy*=.87;n.x=Math.max(60,Math.min(840,n.x+n.vx));n.y=Math.max(35,Math.min(455,n.y+n.vy));}
  }
  draw(){
    // RTT updates and force settling change targets, not the pixels directly.
    // Tweening in time (rather than per update count) keeps motion consistent
    // when probe frequency or frame rate changes.
    const now=performance.now(), dt=Math.min(80,Math.max(0,now-(this.lastDraw||now)));
    this.lastDraw=now;
    const blend=this.reduced?.matches?1:1-Math.exp(-dt/135);
    for(const n of this.nodes){n.renderX+=(n.x-n.renderX)*blend;n.renderY+=(n.y-n.renderY)*blend;}
    const visual=n=>[n.renderX,n.renderY];
    this.world.setAttribute('transform',`translate(${this.camera.x} ${this.camera.y}) scale(${this.camera.k})`);
    const active=new Set();for(let i=1;i<this.route.length;i++)active.add([this.route[i-1],this.route[i]].sort().join('|'));
    for(const {l,line,label} of this.linkViews){const a=this.nodes.find(n=>n.id===l.a),b=this.nodes.find(n=>n.id===l.b);const up=l.up&&a.online&&b.online;const used=up&&active.has([l.a,l.b].sort().join('|'));const [ax,ay]=visual(a),[bx,by]=visual(b);line.setAttribute('x1',ax);line.setAttribute('y1',ay);line.setAttribute('x2',bx);line.setAttribute('y2',by);line.setAttribute('class','graph-edge'+(used?' used':'')+(!up?' down':''));label.setAttribute('x',(ax+bx)/2);label.setAttribute('y',(ay+by)/2-7);label.setAttribute('class','edge-label'+(used?' used':''));label.textContent=up?`${Math.round(l.rtt)} ms`:'—';}
    for(const {n,g} of this.nodeViews){g.setAttribute('transform',`translate(${n.renderX} ${n.renderY})`);g.setAttribute('class','graph-node'+(n.id===this.selected?' selected':'')+(this.route.includes(n.id)?' on-route':'')+(!n.online?' down':'')+(n.id==='you'?' self':''));g.setAttribute('aria-pressed',String(n.id===this.selected));}
  }
}
window.PingGraph=PingGraph;

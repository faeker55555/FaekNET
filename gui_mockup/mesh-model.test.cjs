const { test } = require('node:test');
const assert = require('node:assert/strict');
const { Routes } = require('./mesh-model.js');
function setup() { return new Routes(['you','relay','peer','extra'].map(id=>({id,online:true,relay:true}))); }
function probe(m,path,rtt,start=100) { for(let i=0;i<3;i++) assert(m.observe(path,rtt,start+i*100,true)); }
test('a path needs three fresh validated probes; duplicates cannot satisfy it',()=>{
 const m=setup(),p=['you','peer'];assert(!m.observe(p,10,100,false));assert.equal(m.choose('peer',100),null);
 assert(m.observe(p,10,100,true));assert(!m.observe(p,10,100,true));assert.equal(m.choose('peer',100),null);
 m.observe(p,10,200,true);assert.equal(m.choose('peer',200),null);m.observe(p,10,300,true);assert.equal(m.choose('peer',300).rtt,10);
});
test('faster measured relay switches only after the hold-down',()=>{
 const m=setup();probe(m,['you','peer'],60);assert.equal(m.choose('peer',300).via,null);
 probe(m,['you','relay','peer'],25,400);assert.equal(m.choose('peer',600).via,null);
 probe(m,['you','peer'],60,10100);probe(m,['you','relay','peer'],25,10100);
 assert.equal(m.choose('peer',10300).via,'relay');
});
test('small improvements do not flap a valid direct route',()=>{
 const m=setup();probe(m,['you','peer'],50);m.choose('peer',300);
 probe(m,['you','peer'],50,10100);probe(m,['you','relay','peer'],46,10100);
 assert.equal(m.choose('peer',10300).via,null);
});
test('an unavailable relay falls back immediately to a validated direct path',()=>{
 const m=setup();probe(m,['you','peer'],60);probe(m,['you','relay','peer'],25);
 assert.equal(m.choose('peer',300).via,'relay');m.nodes.get('relay').online=false;
 assert.equal(m.choose('peer',400).via,null);
});
test('withdrawing relay permission invalidates that route',()=>{
 const m=setup();probe(m,['you','relay','peer'],20);assert(m.choose('peer',300));
 m.nodes.get('relay').relay=false;assert.equal(m.choose('peer',400),null);
});
test('loops, unknown peers, excessive hops and invalid RTT are rejected',()=>{
 const m=setup();for(const p of [['you','relay','you'],['you','relay','extra','peer'],['you','missing'],['peer','you'],['you']])assert(!m.observe(p,10,100,true));
 for(const rtt of [0,-1,Infinity,NaN])assert(!m.observe(['you','peer'],rtt,100,true));
});
test('stale observations cannot keep a route alive and new probes restart validation',()=>{
 const m=setup();probe(m,['you','peer'],30);assert.equal(m.choose('peer',10301),null);
 m.observe(['you','peer'],20,11000,true);assert.equal(m.choose('peer',11000),null);
});
test('endpoint invalidation removes all paths involving that identity',()=>{
 const m=setup();probe(m,['you','peer'],60);probe(m,['you','relay','peer'],25);m.invalidate('peer');
 assert.equal(m.choose('peer',301),null);
});

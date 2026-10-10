// Browser checks for the simulated force graph, routing and repository UI.
const assert = require('node:assert/strict');
const path = require('node:path');
const { pathToFileURL } = require('node:url');
const { chromium } = require('playwright');
(async () => {
 const options={headless:true};
 if(process.env.USE_BUNDLED_CHROMIUM==='1'){const bundled=require('@sparticuz/chromium').default;options.executablePath=await bundled.executablePath();options.args=bundled.args;}
 const browser=await chromium.launch(options);
 try {
  const page=await browser.newPage({viewport:{width:1440,height:1250},deviceScaleFactor:1});const errors=[];
  page.on('pageerror',e=>errors.push(e.message));await page.goto(pathToFileURL(path.join(__dirname,'redesign.html')).href);
  await page.evaluate(()=>document.fonts.ready);await page.waitForFunction(()=>window.meshDemo&&document.querySelector('#route-kind').textContent==='Peer relay');
  assert.equal(await page.locator('.peer-row').count(),8);assert.equal(await page.locator('[data-node]').count(),9);
  const text=await page.locator('body').innerText();for(const removed of ['Your network, together.','YOUR OWN LITTLE INTERNET','ENCRYPTED MESH','Pure peer-to-peer'])assert(!text.includes(removed));
  await page.screenshot({path:path.join(__dirname,'redesign-preview.png'),fullPage:true});
  assert.deepEqual(await page.evaluate(()=>meshDemo.graph.route),['you','gaming-pc','alice']);
  await page.locator('#fail-relay').click();assert.equal(await page.locator('#route-kind').innerText(),'Direct path');
  await page.locator('#fail-relay').click();
  await page.locator('#peer-search').fill('10.66.0.4');assert.equal(await page.locator('.peer-row').count(),1);await page.locator('.peer-row').click();assert.equal(await page.locator('#detail-name').innerText(),'laptop');
  await page.locator('#peer-search').fill('not-a-peer');assert.equal(await page.locator('.empty').innerText(),'No matching peers');await page.locator('#reset-map').click();assert.equal(await page.locator('.peer-row').count(),8);
  await page.locator('#motion-toggle').click();assert.equal(await page.evaluate(()=>meshDemo.graph.motion),false);
  const pos=await page.evaluate(()=>({x:meshDemo.nodes[1].x,y:meshDemo.nodes[1].y}));
  const dot=await page.locator('[data-node="alice"] .graph-dot').boundingBox();await page.mouse.move(dot.x+dot.width/2,dot.y+dot.height/2);await page.mouse.down();await page.mouse.move(dot.x+dot.width/2+40,dot.y+dot.height/2+20,{steps:5});await page.mouse.up();
  assert(await page.evaluate(before=>Math.hypot(meshDemo.nodes[1].x-before.x,meshDemo.nodes[1].y-before.y)>10,pos));
  const beforeWeight=await page.evaluate(()=>({x:meshDemo.nodes[1].x,y:meshDemo.nodes[1].y}));
  await page.locator('#rtt-slider').fill('150');await page.locator('#rtt-slider').dispatchEvent('input');
  await page.evaluate(()=>{for(let i=0;i<60;i++)meshDemo.graph.step();meshDemo.graph.draw()});
  assert(await page.evaluate(before=>Math.hypot(meshDemo.nodes[1].x-before.x,meshDemo.nodes[1].y-before.y)>3,beforeWeight));
  await page.locator('#rtt-slider').fill('64');await page.locator('#rtt-slider').dispatchEvent('input');
  await page.locator('#zoom-in').click();assert(await page.evaluate(()=>meshDemo.graph.camera.k>1));await page.locator('#reset-map').click();assert.equal(await page.evaluate(()=>meshDemo.graph.camera.k),1);
  await page.locator('#relay-consent').uncheck();assert.equal(await page.evaluate(()=>meshDemo.nodes.find(n=>n.id==='alice').relay),false);await page.locator('#relay-consent').check();
  const old=await page.evaluate(()=>meshDemo.manifest.get('alice').activeEndpoint);
  await page.evaluate(()=>{window.natTest=meshDemo.simulateNat('alice',true);});await page.waitForFunction(()=>meshDemo.manifest.get('alice').state==='Probing');
  assert.equal(await page.evaluate(()=>meshDemo.manifest.get('alice').activeEndpoint),old);await page.evaluate(()=>window.natTest);assert.equal(await page.evaluate(()=>meshDemo.manifest.get('alice').activeEndpoint),old);
  assert(await page.evaluate(()=>meshDemo.simulateNat('alice',false)));assert.notEqual(await page.evaluate(()=>meshDemo.manifest.get('alice').activeEndpoint),old);assert.equal(await page.evaluate(()=>meshDemo.manifest.get('alice').state),'Installed');
  await page.locator('#toggle-mesh').click();assert.equal(await page.locator('#online-count').innerText(),'0');await page.locator('#toggle-mesh').click();assert.equal(await page.locator('#online-count').innerText(),'7');
  await page.locator('#theme-toggle').click();assert.equal(await page.locator('html').getAttribute('data-theme'),'light');await page.locator('#theme-toggle').click();
  for(const name of ['add','directory','domains','activity','settings']){await page.locator(`[data-dialog="${name}"]`).first().click();assert(await page.locator('#modal').isVisible());await page.keyboard.press('Escape');}
  await page.locator('#copy-address').click();assert(await page.locator('.toast').isVisible());
  for(const width of [1440,980,760,600,390]){await page.setViewportSize({width,height:1000});assert(!(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth)),`Overflow at ${width}`);}
  assert.deepEqual(errors,[]);console.log('PASS: graph, relay fallback, search, dragging, zoom, consent, NAT validation/rejection, pause/resume, themes, dialogs and responsive layouts.');
 } finally {await browser.close();}
})().catch(e=>{console.error(e);process.exit(1)});

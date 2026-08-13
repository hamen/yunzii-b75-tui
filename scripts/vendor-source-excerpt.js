// Excerpt from https://yunzii-game.com/assets/index-8Bj3uPPc.js (2026-08-10).
// Fetched client-side from the page already loaded in the browser (not
// extracted from firmware or any protected asset).
//
// CORRECTION (found in PR #1 round-2 cross-review): an earlier version of
// this file claimed to keep "minified variable names as-is" while actually
// showing deminified names (switchToHomepage, updateDeviceTime, etc.) --
// self-contradictory, and not literally the source text. This version
// includes the actual verbatim minified text FIRST, exactly as fetched, and
// the deminified version SECOND, clearly labeled as a readability aid, not
// as evidence in itself.

// =====================================================================
// VERBATIM (offset ~1828349-1829300 of the fetched bundle, minified as-is)
// =====================================================================

// CRC-16/ARC helper, verbatim:
//   function ga(t){let n=65535;for(let r=0;r<t.length;r++){n^=t[r];for(let i=0;i<8;i++)n&1?(n>>=1,n^=40961):n>>=1}return[n>>8&255,n&255]}

// Equipment-setup button handlers, verbatim:
//   const hDe=()=>{const t=Lu(a=>a.sendScreenControlInformationPackage),n=Lu(a=>a.finishScreenControlDataPacket),r=Lu(a=>a.sendScreenControlDataPacket),i=async()=>{const a=ga([11,0,0]),g=[165,90,11,0,0,a[0],a[1]];await t(g),await n()},o=async()=>{const a=ga([13,0,0]),g=[165,90,13,0,0,a[0],a[1]];await t(g),await n()},c=async()=>{const a=ga([15,0,0]),g=[165,90,15,0,0,a[0],a[1]];await t(g),await n()},d=async()=>{for(let a=0;a<16;a++){const g=ga([14,0,0]),p=[165,90,14,0,0,g[0],g[1]];await t(p),await n()}}, ...

// THE "Update device time" handler, verbatim:
//   f=async()=>{const a=Uf().hour(),g=Uf().minute(),p=Uf().second(),m=Number(Uf().format("YY")),w=Uf().month()+1,x=Uf().date(),S=Uf().day()||7,D=[165,90,9,0,3,195,225],T=[165,90,10,0,4,1,80],P=[a,g,p],M=[m,S,w,x];for(let O=0;O<3;O++)await t(D),await r(P),await n(),await t(T),await r(M),await n()}

// Opcode table (property names in the fetched bundle are NOT minified here --
// this object's keys are used as-is by the site's own code, verbatim):
//   sendScreenControlInformationPackage:"0x40",sendScreenControlDataPacket:"0x41",finishScreenControlDataPacket:"0x42",getDongleAndKeyboardStatus:"0x55",getFirmwareVersion:"0xB0",toBootLoader:"0xB1",getBootLoaderStatus:"0xB2",confirmFirmwareInfo:"0xB3",startUpgrade:"0xB4",transferUpgradeData:"0xB5",upgradeComplete:"0xB6",endUpgrade:"0xB7"

// GIF-save frame pipeline call site, verbatim (re-fetched 2026-08-13 from the
// SAME file -- index-8Bj3uPPc.js, identical byte length -- as the excerpt
// above; offset ~2180990 of that fetch). `Ie` here is the save mode
// (0/1/2), NOT the placement value used elsewhere in this file:
//   Pr=async Ie=>{var et;try{if(he.current&&P.length>0){const Fe=[];y(!0),W($t("oxq2hkb","准备处理GIF帧...","lang")),Q(0);let ke=0;Ie===0?ke=Math.min(P.length,64):Ie===1?ke=Math.min(P.length,160):Ie===2&&(ke=Math.min(P.length,42));const st=Ie===2?96:t,ut=Ie===2?64:n,le=document.createElement("canvas");le.width=st,le.height=ut;const me=le.getContext("2d",{willReadFrequently:!0});me.imageSmoothingEnabled=!1;for(let Ee=0;Ee<ke;Ee++){const Be=Math.floor(Ee/ke*50);Q(Be),W(`${$t("dlmn93","处理帧","lang")}${Ee+1}/${ke}`),await new Promise(xt=>setTimeout(xt,0));const nt=P[Ee];if(nt instanceof jn.fabric.Image){const xt=nt.getElement();nt.filters&&nt.filters.length>0&&(nt.applyFilters(),(et=he.current)==null||et.renderAll());let Kt=Ut(xt,st,ut,D);Kt=pn(Kt),x&&Vr(Kt,.25);const ft=wn(Kt),Dt=Array.from(ft).flatMap(pt=>[pt>>8&255,pt&255]);Fe.push(Dt)}}

// `x` is a plain useState boolean, verbatim (offset ~2171635), and its
// setter is loaded from a saved preset's `sharpen` field elsewhere in the
// same component (offset ~2172652) -- the same control M6 already found
// driving the pre-placement fabric `Convolute` sharpen kernel:
//   [x,S]=j.useState(!1)
//   ... S(Fe.sharpen) ...

// pn(): edge-aware denoise, verbatim (offset ~2185156):
//   function pn(Ie){const et=Ie.width,Fe=Ie.height,ke=Ie.data,st=new Uint8ClampedArray(ke),ut=new ImageData(st,et,Fe),le=2,me=25,Ee=new Array(et*Fe).fill(!1);for(let Be=le;Be<Fe-le;Be++)for(let nt=le;nt<et-le;nt++){const xt=(Be*et+nt)*4;let Kt=!1;for(let ft=0;ft<3;ft++){const Dt=ke[xt-4+ft],pt=ke[xt+4+ft],mt=Math.abs(Dt-pt),yt=ke[xt-et*4+ft],Nt=ke[xt+et*4+ft],gn=Math.abs(yt-Nt);if(mt>me||gn>me){Kt=!0;break}}Ee[Be*et+nt]=Kt}for(let Be=le;Be<Fe-le;Be++)for(let nt=le;nt<et-le;nt++){const xt=(Be*et+nt)*4;let Kt=!1;for(let ft=-1;ft<=1;ft++){for(let Dt=-1;Dt<=1;Dt++)if(Ee[(Be+ft)*et+(nt+Dt)]){Kt=!0;break}if(Kt)break}if(Kt){let ft=0,Dt=0;for(let pt=0;pt<3;pt++)ft+=Math.abs(ke[xt-4+pt]-ke[xt+4+pt]),Dt+=Math.abs(ke[xt-et*4+pt]-ke[xt+et*4+pt]);if(ft>Dt)for(let pt=0;pt<3;pt++){let mt=0,yt=0;for(let Nt=-2;Nt<=le;Nt++){const gn=Be+Nt;if(gn<0||gn>=Fe)continue;const It=(gn*et+nt)*4+pt,mn=le+1-Math.abs(Nt);mt+=ke[It]*mn,yt+=mn}st[xt+pt]=Math.round(mt/yt)}else for(let pt=0;pt<3;pt++){let mt=0,yt=0;for(let Nt=-2;Nt<=le;Nt++){const gn=nt+Nt;if(gn<0||gn>=et)continue;const It=(Be*et+gn)*4+pt,mn=le+1-Math.abs(Nt);mt+=ke[It]*mn,yt+=mn}st[xt+pt]=Math.round(mt/yt)}}else for(let ft=0;ft<3;ft++){const Dt=ke[xt+ft],pt=ke[xt-et*4+ft],mt=ke[xt+et*4+ft],yt=ke[xt-4+ft],Nt=ke[xt+4+ft];st[xt+ft]=Math.round(Dt*.6+(pt+mt+yt+Nt)/4*.4)}}return ut}

// Vr(): edge-aware local-contrast/sharpen, verbatim (offset ~2183721):
//   function Vr(Ie,et=.3){const Fe=Ie.data,ke=Ie.width,st=Ie.height,ut=new Uint8ClampedArray(Fe);for(let le=1;le<st-1;le++)for(let me=1;me<ke-1;me++){const Ee=(le*ke+me)*4;for(let Be=0;Be<3;Be++){const nt=ut[Ee+Be],xt=ut[Ee-ke*4+Be],Kt=ut[Ee-4+Be],ft=ut[Ee+4+Be],Dt=ut[Ee+ke*4+Be],pt=Math.abs(Kt-ft),mt=Math.abs(xt-Dt),yt=pt>40||mt>40;let Nt=et;if(yt)Nt=et*.3;else{const It=Math.min(1,(pt+mt)/50);Nt=et*(1-It*.5)}const gn=(xt+Kt+ft+Dt)/4;Fe[Ee+Be]=Math.max(0,Math.min(255,nt+(nt-gn)*Nt))}}}

// wn(): RGB565 packer with error-diffusion dithering, verbatim (offset
// ~2186483). NOTE the weights on the two `+=` groups: the right-neighbour
// term is `*2/4` (= 1/2) and the two down-row terms are each `*1/4` -- three
// taps total (right, down-left, down), no down-right term. This is NOT the
// classic Floyd-Steinberg coefficients (7/16, 3/16, 5/16, 1/16); it is the
// vendor's own simpler 3-tap scheme in the same directional shape:
//   function wn(Ie){const et=Ie.data,Fe=Ie.width,ke=Ie.height,st=new Uint16Array(Fe*ke),ut=new Array(Fe*ke*3).fill(0);for(let le=0;le<ke;le++)for(let me=0;me<Fe;me++){const Ee=(le*Fe+me)*4,Be=(le*Fe+me)*3;let nt=Math.max(0,Math.min(255,et[Ee]+ut[Be])),xt=Math.max(0,Math.min(255,et[Ee+1]+ut[Be+1])),Kt=Math.max(0,Math.min(255,et[Ee+2]+ut[Be+2]));const ft=Math.min(31,Math.round(nt/255*31)),Dt=Math.min(63,Math.round(xt/255*63)),pt=Math.min(31,Math.round(Kt/255*31)),mt=nt-ft*255/31,yt=xt-Dt*255/63,Nt=Kt-pt*255/31;me<Fe-1&&(ut[Be+3]+=mt*2/4,ut[Be+4]+=yt*2/4,ut[Be+5]+=Nt*2/4),le<ke-1&&(me>0&&(ut[Be+Fe*3-3]+=mt*1/4,ut[Be+Fe*3-2]+=yt*1/4,ut[Be+Fe*3-1]+=Nt*1/4),ut[Be+Fe*3]+=mt*1/4,ut[Be+Fe*3+1]+=yt*1/4,ut[Be+Fe*3+2]+=Nt*1/4),st[le*Fe+me]=ft<<11|Dt<<5|pt}return st}

// The fabric.js editor Canvas construction, verbatim (offset 2148238),
// showing HOW `y.current` (used by Q/I/G below) and `L.current` (the raw
// canvas stage 2 later reads via `drawImage`, see below) relate: fabric's
// `Canvas` constructor WRAPS the existing `L.current` DOM element rather
// than creating a separate one, and passes no smoothing option of its own
// -- so `y.current` and `L.current` are two references into the same
// underlying canvas, but this excerpt does not show whether fabric.js
// sets any 2D-context smoothing flag internally at render time. The
// PLACEMENT MECHANISM below (transform, not pixel algorithm) is settled
// by this evidence; the exact resampling quality of fabric's own render
// of that transform is NOT independently confirmed by anything quoted
// here, and PROTOCOL.md says so explicitly:
//   j.useEffect(()=>{if(!L.current)return;const J=new jn.fabric.Canvas(L.current,{width:t*2,height:n*2});return y.current=J,()=>{J.dispose()}},[t,n])

// Picture path's own placement, verbatim (offset 2143061 of this same
// fetch). `Q`'s callback runs on image upload; `T` is a useState string
// ("0"/"1", same encoding and default "0" as the GIF path's `D`), `P` is
// its setter, `y.current` is the fabric.js editor Canvas (320x192, i.e.
// panel*2) constructed just above:
//   Q=async J=>{const ae=y.current;ae&&jn.fabric.Image.fromURL(J,oe=>{const he=ae.getWidth(),pe=ae.getHeight(),xe=oe.width?Number((he/oe.width).toFixed(2)):1,ve=oe.height?Number((pe/oe.height).toFixed(2)):1,re=Math.min(xe,ve);T==="0"?(oe.scale(re),oe.set({left:oe.width?Number(((he-oe.width*re)/2).toFixed(2)):0,top:oe.height?Number(((pe-oe.height*re)/2).toFixed(2)):0,selectable:!1,evented:!1,hasControls:!1,hasBorders:!1,lockRotation:!0})):oe.set({left:0,top:0,scaleX:xe,scaleY:ve,selectable:!1,evented:!1,hasControls:!1,hasBorders:!1,lockRotation:!0}),ae.clear(),ae.add(oe),ae.renderAll()},{crossOrigin:"anonymous"})}

// Contain/stretch re-layout, re-run whenever the placement toggle or any
// adjustment slider changes (`T==="0"?G():I()` appears after every filter
// mutation in this component), verbatim (offset 2145155 for I, 2145441
// for G -- both immediately follow Q above, in that order, in the fetched
// bundle):
//   I=async()=>{const J=y.current;if(!J)return;J.getObjects().forEach(oe=>{const he=J.getWidth(),pe=J.getHeight(),xe=oe.width?Number((he/oe.width).toFixed(2)):1,ve=oe.height?Number((pe/oe.height).toFixed(2)):1;oe.set({left:0,top:0,scaleX:xe,scaleY:ve}),J.clear(),J.add(oe),J.renderAll()})}
//   G=()=>{const J=y.current;if(!J)return;J.getObjects().forEach(oe=>{const he=J.getWidth(),pe=J.getHeight(),xe=oe.width?Number((he/oe.width).toFixed(2)):1,ve=oe.height?Number((pe/oe.height).toFixed(2)):1,re=Math.min(xe,ve);oe.scale(re),oe.set({left:oe.width?Number(((he-oe.width*re)/2).toFixed(2)):0,top:oe.height?Number(((pe-oe.height*re)/2).toFixed(2)):0}),J.clear(),J.add(oe),J.renderAll()})}

// Picture save (stage 2, already resolved in Milestone 3 -- shown here
// only to make the two-stage pipeline explicit in one place), verbatim
// (offset 2147456 for X, 2147233 for te -- te is defined immediately
// before X in the fetched bundle), including `te()`, the picture path's
// RGB565 packer:
//   function te(J){const ae=J.data,oe=new Uint16Array(J.width*J.height);for(let he=0,pe=0;he<ae.length;he+=4,pe++){const xe=ae[he],ve=ae[he+1],re=ae[he+2],we=xe>>3,De=ve>>2,Te=re>>3,Ue=we<<11|De<<5|Te;oe[pe]=Ue}return oe}
//   X=async()=>{if(y.current){O(!0);const ae=document.createElement("canvas");ae.width=t,ae.height=n;const oe=ae.getContext("2d");if(oe){oe.imageSmoothingEnabled=!1,oe.drawImage(L.current,0,0,t,n);const he=oe.getImageData(0,0,t,n),pe=te(he), ...

// =====================================================================
// DEMINIFIED (readability aid only -- not verbatim evidence; see above
// for the actual source text this is derived from)
// =====================================================================

function ga(t) {
  let n = 65535;
  for (let r = 0; r < t.length; r++) {
    n ^= t[r];
    for (let i = 0; i < 8; i++)
      n & 1 ? (n >>= 1, n ^= 40961) : n >>= 1;
  }
  return [n >> 8 & 255, n & 255];
}

const switchToHomepage = async () => {
  const crc = ga([11, 0, 0]);
  const pkg = [165, 90, 11, 0, 0, crc[0], crc[1]];
  await sendScreenControlInformationPackage(pkg);
  await finishScreenControlDataPacket();
};

const switchToPicturePage = async () => {
  const crc = ga([13, 0, 0]);
  const pkg = [165, 90, 13, 0, 0, crc[0], crc[1]];
  await sendScreenControlInformationPackage(pkg);
  await finishScreenControlDataPacket();
};

const switchToGifPage = async () => {
  const crc = ga([15, 0, 0]);
  const pkg = [165, 90, 15, 0, 0, crc[0], crc[1]];
  await sendScreenControlInformationPackage(pkg);
  await finishScreenControlDataPacket();
};

// CORRECTIVE NOTE (Milestone 2, live capture, 2026-08-11): despite this
// function's name (from the original fetched/deminified excerpt, kept below
// verbatim), the 16x loop applies ONLY to "Clear the picture" (cmd14).
// "Clear GIF" is a DIFFERENT, non-looped sequence (cmd18 then cmd19, each
// sent once) -- see fields.json's unresolved[] entry for cmd18/19 and
// commands.cmd14_clearPicture. This function was never renamed in the
// vendor's own minified bundle either; the name is simply misleading, not
// evidence the two buttons share a handler.
const clearPictureOrGif_loop16x = async () => {
  // for (a=0; a<16; a++) { crc = ga([14,0,0]); pkg = [165,90,14,0,0,crc0,crc1]; ... }
  // (truncated in the fetched excerpt -- opcode 14, sent 16 times)
  // Confirmed by live capture: this is "Clear the picture" ONLY.
};

// "Clear GIF" -- confirmed by live capture (Milestone 2, 2026-08-11) to be
// unrelated to clearPictureOrGif_loop16x above: two different inner
// commands, cmd18 then cmd19, each sent once (no loop). Reconstructed here
// from captured bytes, not fetched from the bundle directly.
const clearGif_notLooped = async () => {
  const crc18 = ga([18, 0, 1]);
  const pkg18 = [165, 90, 18, 0, 1, crc18[0], crc18[1], 1, 0]; // trailing [1,0] meaning unresolved
  await sendScreenControlInformationPackage(pkg18);
  await finishScreenControlDataPacket();

  const crc19 = ga([19, 0, 2]);
  const pkg19 = [165, 90, 19, 0, 2, crc19[0], crc19[1], 1, 0]; // trailing [1,0] meaning unresolved
  await sendScreenControlInformationPackage(pkg19);
  await finishScreenControlDataPacket();
};

// THE ACTUAL "Update device time" HANDLER (dayjs instance called `Uf()`):
const updateDeviceTime = async () => {
  const hour = Uf().hour();
  const minute = Uf().minute();
  const second = Uf().second();
  const year2 = Number(Uf().format("YY"));
  const month = Uf().month() + 1;
  const date = Uf().date();
  const weekday = Uf().day() || 7;

  const D = [165, 90, 9, 0, 3, 195, 225];   // ga([9,0,3]) === [195,225], precomputed
  const T = [165, 90, 10, 0, 4, 1, 80];     // ga([10,0,4]) === [1,80], precomputed
  const P = [hour, minute, second];
  const M = [year2, weekday, month, date];

  for (let i = 0; i < 3; i++) {
    await sendScreenControlInformationPackage(D);
    await sendScreenControlDataPacket(P);
    await finishScreenControlDataPacket();
    await sendScreenControlInformationPackage(T);
    await sendScreenControlDataPacket(M);
    await finishScreenControlDataPacket();
  }
};

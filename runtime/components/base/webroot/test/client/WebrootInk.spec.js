/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
spec({server:false})
feature("markup normalizes to image bounds, paints a copy, and exports independently editable snapshot shapes", {
    expect(){
        const p=browser.newPage()
        p.setContent(`<!doctype html><link rel="stylesheet" href="/libs/quasar/quasar.prod.css"><body><div id="app"></div>
            <script src="/libs/vue/vue.global.prod.js"></script><script src="/js/MoquiLib.js"></script>
            <script src="/js/WebrootInk.js"></script><script>
            const c=document.createElement('canvas');c.width=200;c.height=100;
            const ctx=c.getContext('2d');ctx.fillStyle='white';ctx.fillRect(0,0,200,100);
            window.vm=Vue.createApp(moqui.ink.markup,{src:c.toDataURL(),name:'Source',onSave:b=>window.copy=b}).mount('#app');
            </script></body>`)
        p.waitFor(()=>vm.$refs.image?.naturalWidth===200 && vm.$el.querySelector(".m-ink-input")?.clientWidth>100)
        assert.deepEqual(p.evaluate(()=>moqui.ink.fit(300,300,200,100)),{left:0,top:75,width:300,height:150})
        p.evaluate(()=>{vm.strokes.push({id:"pinned",tool:"rectangle",points:[[.25,.25],[.75,.75]],color:"#ff0000",width:.05,pinned:true});return true})
        p.getByRole("button",{name:"Save a copy"}).click()
        p.waitFor(()=>!!window.copy)
        const result=p.evaluate(async()=>{
            const bmp=await createImageBitmap(copy),c=document.createElement("canvas");c.width=200;c.height=100;
            const ctx=c.getContext("2d");ctx.drawImage(bmp,0,0);const original=vm.$refs.image;
            const contrast=ctx.getImageData(50,25,1,1).data[1];ctx.drawImage(original,0,0);
            const snapshot=await moqui.ink.snapshot(original,vm.strokes,false)
            return {red:contrast===0,white:ctx.getImageData(50,25,1,1).data[1]===255,els:snapshot.scene.elements.map(e=>[e.type,e.locked,e.fileId || ""])}
        })
        assert.deepEqual(result,{red:true,white:true,els:[["image",true,"capture"],["rectangle",false,""]]})
    }
})

feature("a late join keeps the stroke's remaining lifetime and rejects an old presentation epoch", {
    expect() {
        const page=browser.newPage()
        page.setContent(`<!doctype html><body><script src="/js/MoquiLib.js"></script><script src="/js/WebrootCall.js"></script></body>`)
        page.evaluate(()=>{
            const call=moqui.call, state=call.state
            state.stage={kind:"screen",inkEpoch:4,callLegId:"A"}
            const stroke={id:"line1",kind:"path",tool:"line",color:"#cc0000",points:[[0,0],[1,1]],width:.01}
            call.inkFrame({action:"stroke",inkEpoch:3,stroke})
            window.oldIgnored=state.inkStrokes.length===0
            call.inkFrame({action:"stroke",inkEpoch:4,ageMs:4960,stroke})
            window.replayed=state.inkStrokes.length===1
            return true
        })
        assert.equal(page.evaluate(()=>oldIgnored && replayed),true)
        page.waitFor(()=>moqui.call.state.inkStrokes.length===0,{timeout:1})
    }
})

feature("embedded ink uses video frame coordinates after capture clipping, scrolling and resizing", {
    expect() {
        const page=browser.newPage()
        page.setContent(`<!doctype html><body style="margin:0">
            <div id="clip" style="position:absolute;top:15px;left:10px;width:540px;height:340px;overflow:auto">
                <div id="target" style="position:relative;width:530px;height:460px"></div></div>
            <script src="/libs/vue/vue.global.prod.js"></script><script src="/js/MoquiLib.js"></script><script src="/js/WebrootInk.js"></script>
            <script>
            const track={getSettings:()=>({width:788,height:600})};
            const stroke={id:"h",tool:"line",color:"#ff0000",points:[[.1,.34],[.9,.34]],width:.004};
            moqui.ink.embedded(target,[stroke],track);
            </script></body>`)
        const geometry=page.evaluate(()=>{
            const overlay=document.querySelector(".m-embedded-ink"),line=overlay.querySelector("line");
            return {w:overlay.clientWidth,h:overlay.clientHeight,y:Number(line.getAttribute("y1")),clip:clip.clientHeight,v:innerHeight}
        })
        assert.ok(Math.abs(geometry.w/geometry.h-788/600)<.01 && Math.abs(geometry.y-geometry.h*.34)<.5,JSON.stringify(geometry))
        page.evaluate(()=>{clip.scrollTop=60;clip.style.height="250px";return true})
        page.waitFor(()=>parseFloat(document.querySelector(".m-embedded-ink").style.top)>0,{timeout:2})
        const resized=page.evaluate(()=>{
            const el=document.querySelector(".m-embedded-ink"),r=el.getBoundingClientRect();
            return {w:r.width,h:r.height,y:Number(el.querySelector("line").getAttribute("y1")),top:r.top,clipTop:clip.getBoundingClientRect().top,
                scroll:clip.scrollTop,viewport:[innerWidth,innerHeight]}
        })
        assert.ok(resized.h<geometry.h && Math.abs(resized.top-resized.clipTop)<1 && Math.abs(resized.y-resized.h*.34)<.5,JSON.stringify(resized))

    }
})

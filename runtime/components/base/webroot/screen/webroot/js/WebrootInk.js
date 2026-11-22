/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
(function (root) {
    "use strict";
    const {h, markRaw} = root.Vue, moqui = root.moqui;
    const ink = moqui.ink = {};
    const t = key => moqui.translate ? moqui.translate(key) : key;
    const clamp = n => Math.max(0, Math.min(1, n));
    const svgNS = "http://www.w3.org/2000/svg";
    const absolute = {position:"absolute", inset:0, width:"100%", height:"100%"};
    const tools = ["draw","line","rectangle","ellipse"];
    const css=document.createElement("style");
    css.textContent="@keyframes m-ink-fade {from{opacity:1}to{opacity:0}}";
    document.head.appendChild(css);
    function fade(stroke) {
        if(stroke.pinned || !stroke.startedAt)return "";
        const elapsed=Date.now()-stroke.startedAt, laser=stroke.kind==="laser";
        return "animation:m-ink-fade "+(laser?500:450)+"ms ease-out "+((laser?100:4550)-elapsed)+"ms both";
    }
    ink.fit = (w,h,sw,sh) => {
        if (!sw || !sh || !w || !h) return {left:0,top:0,width:w,height:h};
        const ratio=Math.min(w/sw,h/sh), width=sw*ratio,height=sh*ratio;
        return {left:(w-width)/2,top:(h-height)/2,width,height};
    };
    function geometry(stroke,w,h) {
        const points=stroke.points.map(([x,y])=>[x*w,y*h]), [x,y]=points[0], [xx,yy]=points[points.length-1];
        const bounds={x:Math.min(x,xx),y:Math.min(y,yy),width:Math.abs(xx-x),height:Math.abs(yy-y)};
        const attrs={style:fade(stroke),stroke:stroke.color,"stroke-width":Math.max(.5,stroke.width*Math.min(w,h)),fill:"none","stroke-linecap":"round","stroke-linejoin":"round"};
        if (stroke.tool==="rectangle") return ["rect",{...attrs,...bounds}];
        if (stroke.tool==="ellipse") return ["ellipse",{...attrs,cx:(x+xx)/2,cy:(y+yy)/2,rx:bounds.width/2,ry:bounds.height/2}];
        if (points.length===1) return ["circle",{style:fade(stroke),cx:x,cy:y,r:attrs["stroke-width"]/2,fill:stroke.color}];
        if (stroke.tool==="line") return ["line",{...attrs,x1:x,y1:y,x2:xx,y2:yy}];
        return ["polyline",{...attrs,points:points.map(p=>p.join(",")).join(" ")}];
    }
    function svgShapes(strokes,w,h) {
        return strokes.map(s=>{const [tag,attrs]=geometry(s,w,h);return Vue.h(tag,{...attrs,key:s.id,"data-stroke":s.id});});
    }
    // Element/Region Capture sends the visible intersection of the element and its clipping
    // ancestors. The page itself may be taller than the viewport or scrolled off the top.
    ink.captureBox = (target, track) => {
        const view=root.visualViewport;
        const left=view?.offsetLeft || 0,top=view?.offsetTop || 0;
        let x=left,y=top,r=left+(view?.width || root.innerWidth),b=top+(view?.height || root.innerHeight);
        const body=target===document.body;
        const bounds=target.getBoundingClientRect();
        if(!body){
            x=Math.max(x,bounds.left);y=Math.max(y,bounds.top);r=Math.min(r,bounds.right);b=Math.min(b,bounds.bottom);
            for(let p=target.parentElement;p && p!==document.body;p=p.parentElement){
                const rect=p.getBoundingClientRect(),style=getComputedStyle(p);
                if(style.overflowX!=="visible"){x=Math.max(x,rect.left+p.clientLeft);r=Math.min(r,rect.left+p.clientLeft+p.clientWidth);}
                if(style.overflowY!=="visible"){y=Math.max(y,rect.top+p.clientTop);b=Math.min(b,rect.top+p.clientTop+p.clientHeight);}
            }
        }
        let width=Math.max(0,r-x),height=Math.max(0,b-y);
        const settings=track?.getSettings?.(),ratio=settings?.width/settings?.height;
        if(ratio>0 && width && height){
            // Chromium rounds crop dimensions to codec pixels. When it additionally clips
            // the bottom or right, the actual track dimensions define the drawable bounds.
            if(width/height>ratio)width=height*ratio;
            else height=width/ratio;
        }
        return {left:body?x:x-bounds.left-target.clientLeft+target.scrollLeft,top:body?y:y-bounds.top-target.clientTop+target.scrollTop,width,height};
    };
    ink.embedded = (target,strokes,track) => {
        if (ink.target !== target) {
            ink.overlay?.remove();ink.resizeOff?.();ink.overlay=null;ink.target=target;
            if (target) {
                const el=document.createElementNS(svgNS,"svg");
                el.classList.add("m-embedded-ink");
                Object.assign(el.style,{pointerEvents:"none",zIndex:2147483646,position:target===document.body?"fixed":"absolute"});
                if (target!==document.body && getComputedStyle(target).position==="static") target.style.position="relative";
                target.appendChild(el);ink.overlay=el;
                let frame=0;
                const resize=()=>{if(!frame)frame=requestAnimationFrame(()=>{
                    frame=0;if(ink.target===target)ink.embedded(target,ink.visible || [],ink.track);
                });};
                const observer=new ResizeObserver(resize);
                for(let p=target;p;p=p.parentElement)observer.observe(p);
                root.addEventListener("resize",resize);root.addEventListener("scroll",resize,true);
                root.visualViewport?.addEventListener("resize",resize);root.visualViewport?.addEventListener("scroll",resize);
                ink.resizeOff=()=>{
                    cancelAnimationFrame(frame);observer.disconnect();root.removeEventListener("resize",resize);root.removeEventListener("scroll",resize,true);
                    root.visualViewport?.removeEventListener("resize",resize);root.visualViewport?.removeEventListener("scroll",resize);ink.resizeOff=null;
                };
            }
        }
        ink.visible=strokes;ink.track=track;
        const el=ink.overlay;
        if (!el) return;
        const {left,top,width:w,height:h}=ink.captureBox(target,track);
        Object.assign(el.style,{left:left+"px",top:top+"px",width:w+"px",height:h+"px"});
        el.replaceChildren(...strokes.map(s=>{
            const [tag,attrs]=geometry(s,w,h), shape=document.createElementNS(svgNS,tag);
            for (const [k,v] of Object.entries(attrs)) shape.setAttribute(k,v);
            return shape;
        }));
    };
    function blobFrom(canvas) {
        return new Promise((resolve,reject)=>canvas.toBlob(blob=>blob?resolve(blob):reject(new Error(t("Image export failed"))),"image/png"));
    }
    async function pixels(media,strokes) {
        const w=media.videoWidth || media.naturalWidth,h=media.videoHeight || media.naturalHeight;
        if (!w || !h) throw new Error(t("The image is loading"));
        const canvas=document.createElement("canvas");canvas.width=w;canvas.height=h;
        const ctx=canvas.getContext("2d");ctx.drawImage(media,0,0,w,h);
        for (const s of strokes) {
            const pts=s.points.map(([x,y])=>[x*w,y*h]), [a,b]=pts[0], [c,d]=pts.at(-1);
            ctx.strokeStyle=ctx.fillStyle=s.color;ctx.lineWidth=Math.max(.5,s.width*Math.min(w,h));ctx.lineJoin=ctx.lineCap="round";ctx.beginPath();
            if (s.tool==="rectangle") ctx.rect(a,b,c-a,d-b);
            else if (s.tool==="ellipse") ctx.ellipse((a+c)/2,(b+d)/2,Math.abs(c-a)/2,Math.abs(d-b)/2,0,0,Math.PI*2);
            else if (pts.length===1) {ctx.arc(a,b,ctx.lineWidth/2,0,Math.PI*2);ctx.fill();continue;}
            else {ctx.moveTo(a,b);for(const p of s.tool==="line"?[[c,d]]:pts.slice(1))ctx.lineTo(...p);}
            ctx.stroke();
        }
        return blobFrom(canvas);
    }
    // The captured pixels and every annotation are separate board elements.
    ink.snapshot = async (video,strokes,embedded) => {
        const overlay=embedded && ink.overlay, w=video.videoWidth || video.naturalWidth,h=video.videoHeight || video.naturalHeight;
        if (embedded && !overlay) throw new Error(t("The presenter can save this tab snapshot"));
        let frame;
        try {
            if (overlay) {
                overlay.style.visibility="hidden";
                // Wait for the screen capture to encode and our video to present the clean frame.
                await new Promise((resolve,reject)=>{
                    if (!video.requestVideoFrameCallback) return setTimeout(resolve,200);
                    const timeout=setTimeout(()=>{video.cancelVideoFrameCallback(token);reject(new Error(t("A fresh video frame is unavailable")));},1800);
                    const token=video.requestVideoFrameCallback(()=>{clearTimeout(timeout);resolve();});
                });
            }
            frame=await pixels(video,[]);
        } finally {if (overlay) overlay.style.visibility="";}
        await moqui.loadScript("/libs/excalidraw/editor.js",null,()=>!!root.MoquiBoardEditor);
        const skeleton=[{type:"image",x:0,y:0,width:w,height:h,fileId:"capture",status:"saved",locked:true}];
        for (const s of strokes.filter(s=>s.kind!=="laser")) {
            const pts=s.points.map(([x,y])=>[x*w,y*h]), [x,y]=pts[0], [xx,yy]=pts.at(-1);
            const style={strokeColor:s.color,strokeWidth:Math.max(1,s.width*Math.min(w,h)),roughness:0};
            if (s.tool==="rectangle" || s.tool==="ellipse")
                skeleton.push({...style,type:s.tool,x:Math.min(x,xx),y:Math.min(y,yy),width:Math.max(1,Math.abs(xx-x)),height:Math.max(1,Math.abs(yy-y))});
            else {
                const line=s.tool==="line"?[pts[0],pts.at(-1)]:pts;
                const relative=line.map(([xx,yy])=>[xx-x,yy-y]);
                if(relative.length===1)relative.push([.1,.1]);
                skeleton.push({...style,type:"line",x,y,points:relative});
            }
        }
        const elements=root.MoquiBoardEditor.convertToExcalidrawElements(skeleton);
        return {frame,scene:{type:"excalidraw",version:2,elements,appState:{viewBackgroundColor:"#ffffff"},files:{}}};
    };
    ink.layer = {
        name:"mInkLayer",props:{strokes:Array,active:Boolean,tool:{type:String,default:"draw"},color:{type:String,default:"#e53935"},pin:Boolean,hiddenEcho:Boolean},
        emits:["stroke"],data(){return {box:{left:0,top:0,width:0,height:0},error:""};},
        mounted() {
            this.measure=this.resize.bind(this);this.observer=new ResizeObserver(this.measure);this.observer.observe(this.$el);
            this.media=this.$el.parentNode.querySelector("video,img");
            for(const e of ["load","loadedmetadata","resize"])this.media?.addEventListener(e,this.measure);
            this.resize();if(this.active)this.start();
        },
        beforeUnmount(){this.gone=true;this.drauu?.unmount();this.observer?.disconnect();
            for(const e of ["load","loadedmetadata","resize"])this.media?.removeEventListener(e,this.measure);},
        watch:{active(v){if(v)this.start();else{this.drauu?.unmount();this.drauu=null;this.$refs.input?.replaceChildren();}},tool:"brush",color:"brush"},
        methods:{
            resize(){const m=this.media;this.box=ink.fit(this.$el.clientWidth,this.$el.clientHeight,m?.videoWidth || m?.naturalWidth,m?.videoHeight || m?.naturalHeight);},
            brush(){if(this.drauu)this.drauu.brush={mode:this.tool==="laser"?"draw":this.tool,color:this.color,size:3};},
            async start(){
                try {
                    const {createDrauu}=await import("/libs/drauu/drauu.js");
                    if(this.gone || !this.active || this.drauu)return;
                    this.drauu=markRaw(createDrauu({el:this.$refs.input}));this.brush();
                    this.drauu.on("end",()=>{
                        const points=this.samples || [];
                        if(points.length && this.tool!=="laser"){
                            const every=Math.max(1,Math.ceil(points.length/511));
                            const sampled=points.filter((p,i)=>i%every===0);if(sampled.at(-1)!==points.at(-1))sampled.push(points.at(-1));
                            this.$emit("stroke",{id:crypto.randomUUID(),kind:this.tool==="laser"?"laser":"path",tool:this.tool==="laser"?"draw":this.tool,
                                points:sampled,color:this.color,width:Math.min(.1,3/Math.max(1,Math.min(this.box.width,this.box.height))),pinned:this.pin && this.tool!=="laser"});
                        }
                        this.samples=null;this.laserId=null;this.drauu.clear();
                    });
                } catch(error){this.error=String(error.message || error);}
            },
            sample(event,start){
                if (!this.active || !this.drauu || (!start && (!this.samples || event.pointerId!==this.pointer)))return;
                if(start){if(this.samples || event.button!==0)return;this.samples=[];this.pointer=event.pointerId;this.$refs.input.setPointerCapture(event.pointerId);}
                const r=this.$refs.input.getBoundingClientRect();
                const p=[clamp((event.clientX-r.left)/r.width),clamp((event.clientY-r.top)/r.height)];
                const last=this.samples.at(-1);if(!last || last[0]!==p[0] || last[1]!==p[1])this.samples.push(p);
                if(this.tool==="laser" && (start || performance.now()-this.laserAt>90)){
                    if(start)this.laserId=crypto.randomUUID();
                    this.laserAt=performance.now();this.$emit("stroke",{id:this.laserId,kind:"laser",tool:"draw",points:this.samples.slice(-4),
                        color:this.color,width:Math.min(.1,5/Math.max(1,Math.min(this.box.width,this.box.height))),pinned:false});
                }
            }
        },
        render(){
            const b=this.box, box={position:"absolute",left:b.left+"px",top:b.top+"px",width:b.width+"px",height:b.height+"px"};
            return h("div",{class:"m-ink-layer",style:{...absolute,pointerEvents:"none"}},[
                h("svg",{style:box},this.hiddenEcho?[]:svgShapes(this.strokes || [],b.width,b.height)),
                h("svg",{ref:"input",class:"m-ink-input",style:{...box,touchAction:"none",pointerEvents:this.active?"auto":"none",cursor:this.active?"crosshair":"default"},
                    onPointerdownCapture:e=>this.sample(e,true),onPointermoveCapture:e=>this.sample(e,false),onPointerupCapture:e=>this.sample(e,false)}),
                this.error?h("span",{role:"alert"},this.error):null]);
        }
    };
    ink.markup = {
        name:"mImageMarkup",props:{src:String,name:String,busy:Boolean},emits:["save","close"],data(){return {strokes:[],tool:"draw",color:"#e53935",error:"",saving:false};},
        methods:{async save(){if(this.saving || this.busy)return;this.saving=true;
            try{this.$emit("save",await pixels(this.$refs.image,this.strokes));}catch(e){this.error=String(e.message || e);}finally{this.saving=false;}}},
        render(){
            const btn=(label,click,disabled)=>h("button",{class:"q-btn q-btn--flat q-px-sm",type:"button",disabled,onClick:click},t(label));
            return h("div",{class:"m-image-markup column bg-dark text-white",role:"dialog","aria-label":t("Mark up image"),style:{position:"fixed",inset:0,zIndex:6500}},[
                h("div",{class:"row items-center q-pa-sm q-gutter-xs"},[h("strong",this.name),
                    h("select",{"aria-label":t("Drawing tool"),value:this.tool,onChange:e=>this.tool=e.target.value},tools.map(n=>h("option",{value:n},t(n)))),
                    h("input",{type:"color","aria-label":t("Ink color"),value:this.color,onInput:e=>this.color=e.target.value}),
                    btn("Undo",()=>this.strokes.pop(),!this.strokes.length),btn("Save a copy",this.save,!this.strokes.length || this.busy || this.saving),btn("Close",()=>this.$emit("close"))]),
                h("div",{class:"col relative-position",style:{minHeight:0}},[h("img",{ref:"image",src:this.src,alt:this.name,style:{...absolute,objectFit:"contain"}}),
                    h(ink.layer,{active:true,strokes:this.strokes,tool:this.tool,color:this.color,pin:true,onStroke:s=>this.strokes.push(s)})]),
                this.error?h("span",{role:"alert"},this.error):null]);
        }
    };
})(window);

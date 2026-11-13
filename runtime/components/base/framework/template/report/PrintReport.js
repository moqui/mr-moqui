/* CC0. Offline report rendering uses the same panels and formatting as the browser. */
(async function () {
    try {
        const h = Vue.h;
        const chartComponent = {
            props: ['config', 'height'],
            mounted() {
                const config = this.config;
                config.options = Object.assign({}, config.options, {animation:false, responsive:false});
                this.chart = new Chart(this.$refs.canvas, config);
            },
            beforeUnmount() { if (this.chart) this.chart.destroy(); },
            render() { return h('div', {class:'print-chart'}, [h('canvas', {ref:'canvas',width:640,height:260})]); }
        };
        const panel = (id, value, style) => h('article', {key:id,style}, [
            h(moqui.qvtComponents['m-report-panel'], {panel:value,frames:value.frames || [],title:value.title || id}),
            ...(value.notices || []).map(notice=>h('p', {class:'notice'}, notice.message || String(notice)))
        ]);
        const allPanels = dashboard => Object.entries(dashboard.panels || {}).map(([id,value])=>panel(id,value));
        const layout = (node,dashboard,depth=0) => {
            if (depth>16) throw new Error('Report layout nesting exceeds limit');
            if (!node) return h('div',{class:'print-auto'},allPanels(dashboard));
            if (typeof node==='string') node={panelId:node};
            if (node.panelId) return dashboard.panels[node.panelId] ? panel(node.panelId,dashboard.panels[node.panelId]) : null;
            const items=node.items || node.children || [];
            if (node.type==='grid') return h('div',{class:'print-grid'},items.map(item=>dashboard.panels[item.panelId] ?
                panel(item.panelId,dashboard.panels[item.panelId],{gridColumn:(Math.max(0,Math.min(23,item.x || 0))+1)+'/span '+Math.max(1,Math.min(24,item.w || 12)),
                    gridRow:(Math.max(0,item.y || 0)+1)+'/span '+Math.max(1,item.h || 1),minWidth:0}) : null));
            if (['rows','row','tabs'].includes(node.type)) return h('div',items.map(item=>h('section',[
                h('h3',item.title || ''),layout(item.layout || {type:'autoGrid',items:item.items || []},dashboard,depth+1)])));
            return h('div',{class:'print-auto'},items.length ? items.map(item=>layout(item,dashboard,depth+1)) : allPanels(dashboard));
        };
        const table = (frame) => {
            const fields=moqui.report.fields(frame), rows=moqui.report.rows(frame);
            return h('table',[h('thead',[h('tr',fields.map(field=>h('th',field.displayName || field.name)))]),
                h('tbody',rows.map(row=>h('tr',row.map((value,at)=>h('td',moqui.report.format(value,fields[at]))))))]);
        };
        const app=Vue.createApp({render() {
            return h('main',moquiPrintData.dashboards.map(dashboard=>h('section',{class:'print-dashboard'},[
                dashboard.title!==moquiPrintData.title ? h('h2',dashboard.title || 'Dashboard') : null,
                h('p',{class:'range'},moqui.report.format(dashboard.range.from,{type:'time'})+' – '+moqui.report.format(dashboard.range.thru,{type:'time'})),
                moquiPrintData.rows ? h('div',allPanels(dashboard)) : layout(dashboard.layout,dashboard),
                moquiPrintData.appendix ? h('section',{class:'appendix'},[h('h2','Table data'),
                    ...Object.entries(dashboard.panels || {}).map(([id,value])=>h('section',[h('h3',value.title || id),...(value.frames || []).map(table)]))]) : null
            ])));
        }});
        app.component('m-chart',chartComponent);
        app.config.errorHandler = error => { window.moquiReportError=String(error && error.stack || error); };
        app.component('m-markdown',{props:['text'],render(){return h('div',{class:'report-text',
            innerHTML:DOMPurify.sanitize(marked.parse(this.text || ''))});}});
        app.mount('#report');
        await Vue.nextTick();
        await document.fonts.ready;
        await new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)));
        if (window.moquiReportError) throw new Error(window.moquiReportError);
        window.moquiReportRendered=true;
    } catch(error) { window.moquiReportError=String(error && error.stack || error); }
})();

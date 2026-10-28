/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
"use strict";
const report = moqui.report;
test("columnar frames retain ragged nulls and CSV protects spreadsheet formulas", () => {
    const frame = { fields: [{ name: "name", type: "string", values: ['=SUM(A1)', 'a,"b'] },
        { name: "amount", type: "number", values: [-2] }] };
    assert.deepEqual(report.rows(frame), [['=SUM(A1)', -2], ['a,"b', null]]);
    assert.equal(report.csv(frame), '"name","amount"\r\n"\'=SUM(A1)","-2"\r\n"a,""b",""');
    assert.equal(report.csv({fields:[{name:'numeric',type:'number',values:['-0.3']},{name:'text',type:'string',values:['-2+CMD()']}]}), '"numeric","text"\r\n"-0.3","\'-2+CMD()"');
    assert.equal(report.format(null, {}), "—");
    assert.equal(report.format(null, { noValue: "Missing" }), "Missing");
});
test("chart configs use the frame and retain missing intervals", () => {
    const config = report.chart({ fields: [
        { name: "day", type: "string", values: ["Mon", "Tue", "Wed"] },
        { name: "total", type: "number", values: [1, null, 3] },
    ] }, { visualization: "area", stacked: true });
    assert.equal(config.type, "line");
    assert.deepEqual(config.data.labels, ["Mon", "Tue", "Wed"]);
    assert.deepEqual(config.data.datasets[0].data, [1, null, 3]);
    assert.equal(config.data.datasets[0].spanGaps, false);
    assert.equal(config.data.datasets[0].fill, true);
    assert.equal(config.options.animation, false);
});
test("waterfalls preserve nanosecond order and identify missing parents", () => {
    const spans = [
        { span_id: "child", parent_span_id: "root", name: "service", start_nano: "1800000000000000010", duration_nano: 10 },
        { span_id: "root", name: "request", start_nano: "1800000000000000000", duration_nano: 100 },
        { span_id: "orphan", parent_span_id: "missing", name: "async", start_nano: "1800000000000000050", duration_nano: 10 },
    ];
    const rows = report.waterfall(spans);
    assert.equal(rows[0].span.span_id, "root");
    assert.equal(rows[1].depth, 1);
    assert.equal(rows[1].left, 10);
    assert.equal(rows[1].width, 10);
    assert.equal(rows[2].missingParent, true);
    assert.deepEqual(report.waterfall([]), []);
});
test("malformed cyclic ancestry is bounded", () => {
    const rows = report.waterfall([{ span_id: "a", parent_span_id: "b", start_nano: 0, duration_nano: 1 },
        { span_id: "b", parent_span_id: "a", start_nano: 0, duration_nano: 1 }]);
    assert.equal(rows[0].depth, 1);
    assert.equal(rows[1].depth, 1);
});
test("report widgets render with Vue runtime and escape document values", () => {
    const target = document.createElement("div"); document.body.appendChild(target);
    const app = Vue.createApp({ render() { return Vue.h(moqui.qvtComponents["m-report-panel"], {
        panel: { title: "<img src=x onerror=alert(1)>", visualization: "table" },
        frame: { fields: [{ name: "name", type: "string", values: ["<script>bad()</script>"] }] },
    }); } });
    app.mount(target);
    assert.equal(target.querySelector("img"), null);
    assert.equal(target.querySelector("script"), null);
    assert.equal(target.textContent.includes("<script>bad()</script>"), true);
    app.unmount(); target.remove();
});
test("data links reject executable and protocol-relative URLs", () => {
    assert.equal(report.safeLink("javascript:alert(1)"), "");
    assert.equal(report.safeLink("//foreign.example/path"), "");
    assert.equal(report.safeLink("/apps/monitor/Traces"), "/apps/monitor/Traces");
});
test("field overrides, thresholds and links preserve typed values", () => {
    const fields = report.configuredFields({ fields: [{ name: "latency", type: "number", values: [2] }] }, {
        fieldConfig: { defaults: { decimals: 1 }, overrides: [{ match: { name: "latency" }, config: { unit: "s", thresholds: [{ value: 1, color: "negative" }] } }] },
    });
    assert.equal(fields[0].unit, "s");
    assert.equal(fields[0].decimals, 1);
    assert.equal(report.thresholdColor(2, fields[0]), "var(--q-negative)");
    assert.equal(report.dataLink("/apps/orders?id=${value}&status=${status}", "A&B", { status: "On hold" }), "/apps/orders?id=A%26B&status=On%20hold");
    assert.equal(report.thresholdColor(null, fields[0]), "");
    assert.equal(report.format("2026-01-02T03:04:05Z", { type: "time" }).includes("Invalid"), false);
});
test("sparklines retain gaps and bounded log views deduplicate entries", () => {
    const path = report.sparkline([1, null, 3]);
    assert.equal((path.match(/M/g) || []).length, 2);
    const first = { documentId: "otel_logs", id: "a", body: { body: "first" } };
    const second = { documentId: "otel_logs", id: "b", body: { body: "second" } };
    const third = { documentId: "otel_logs", id: "c", body: { body: "third" } };
    const merged = report.mergeLogHits([first, second], [second, third], 2);
    assert.deepEqual(merged.hits.map((hit) => hit.id), ["b", "c"]);
    assert.equal(merged.removed, 1);
});
test("dashboard tabs and 24-column layouts render the selected panel", async () => {
    const target = document.createElement("div"); document.body.appendChild(target);
    const dashboard = { panels: { first: { title: "First panel", visualization: "stat" }, second: { title: "Second panel", visualization: "stat" } },
        layout: { type: "tabs", items: [{ title: "One", layout: { type: "grid", items: [{ panelId: "first", x: 0, w: 12 }] } },
            { title: "Two", layout: { type: "autoGrid", items: [{ panelId: "second" }] } }] } };
    const frame = { fields: [{ name: "value", type: "number", values: [12] }] };
    const app = Vue.createApp({ render() { return Vue.h(moqui.qvtComponents["m-dashboard"], { dashboard, frames: { first: [frame], second: [frame] } }); } });
    app.mount(target);
    assert.equal(target.textContent.includes("First panel"), true);
    assert.equal(target.textContent.includes("Second panel"), false);
    assert.equal(target.querySelector('[role="tabpanel"] > div').style.gridTemplateColumns.includes("24"), true);
    target.querySelectorAll('[role="tab"]')[1].click(); await Vue.nextTick();
    assert.equal(target.textContent.includes("Second panel"), true);
    assert.equal(target.textContent.includes("First panel"), false);
    app.unmount(); target.remove();
});
test("panel option edits and unlinking a library reference update the draft until save", async () => {
    const target = document.createElement("div"); document.body.appendChild(target);
    const form = { fields: Vue.reactive({ definitionJson: JSON.stringify({ panels: { first: { title: "Original", visualization: "table", queries: [{ datasetId: "otel_logs", mode: "documents" }] } } }) }) };
    const library = { id: "library-test", title: "Shared panel", body: { title: "Shared panel", visualization: "stat", queries: [] } };
    const app = Vue.createApp({ provide() { return { mForm: form }; }, render() { return Vue.h(moqui.qvtComponents["m-report-definition-editor"], { libraries: [library], datasets: [{ id: "otel_logs" }] }); } });
    app.mount(target);
    const title = Array.from(target.querySelectorAll("label")).find((label) => label.textContent === "Panel title").querySelector("input");
    title.value = "Edited title"; title.dispatchEvent(new Event("change")); await Vue.nextTick();
    assert.equal(JSON.parse(form.fields.definitionJson).panels.first.title, "Edited title");
    Array.from(target.querySelectorAll('button')).find(button=>button.textContent==='Add aggregation').click();await Vue.nextTick();
    assert.equal(Array.isArray(JSON.parse(form.fields.definitionJson).panels.first.queries[0].aggregations),true);
    assert.equal(JSON.parse(form.fields.definitionJson).panels.first.queries[0].mode,'aggregations');
    Array.from(target.querySelectorAll('button')).find(button=>button.textContent==='Add transformation').click();await Vue.nextTick();
    assert.equal(JSON.parse(form.fields.definitionJson).panels.first.transformations[0].count,100);
    const layout = Array.from(target.querySelectorAll("label")).find((label) => label.textContent.startsWith("Layout ")).querySelector("select");
    layout.value = "grid"; layout.dispatchEvent(new Event("change")); await Vue.nextTick();
    assert.equal(JSON.parse(form.fields.definitionJson).layout.type, "grid");
    const pick = Array.from(target.querySelectorAll("label")).find((label) => label.textContent.startsWith("Library panel ")).querySelector("select");
    pick.value = library.id; pick.dispatchEvent(new Event("change")); await Vue.nextTick();
    Array.from(target.querySelectorAll("button")).find((button) => button.textContent === "Add library reference").click(); await Vue.nextTick();
    const body = JSON.parse(form.fields.definitionJson), id = Object.keys(body.panels).find((id) => body.panels[id].panelRef === library.id);
    assert.equal(!!id, true);
    Array.from(target.querySelectorAll("button")).find((button) => button.textContent === "Unlink as copy").click(); await Vue.nextTick();
    const unlinked = JSON.parse(form.fields.definitionJson).panels[id];
    assert.equal(unlinked.panelRef, undefined);
    assert.equal(unlinked.title, "Shared panel");
    assert.equal(library.body.title, "Shared panel");
    app.unmount(); target.remove();
});
test("annotations map to chart positions while out-of-range markers stay outside the view", () => {
    const frame = { fields: [{ name: "time", type: "time", values: [1000, 2000, 3000] }, { name: "requests", type: "number", values: [1, 2, 3] }] };
    const annotations = [{ time: 2000, time_end: 3000, text: "Deployment" }, { time: 500, text: "Earlier" }];
    const markers = report.annotationPositions(frame, annotations);
    assert.equal(markers.length, 1);
    assert.equal(markers[0].start, 1);
    assert.equal(markers[0].end, 2);
    assert.equal(report.chart(frame, { annotations }).plugins[0].id, "moquiAnnotations");
});
test("panel export keeps exact range and variables and decodes server XLSX bytes", async () => {
    const target=document.createElement('div');document.body.appendChild(target);
    const previousFetch=window.fetch,previousCreate=URL.createObjectURL,previousRevoke=URL.revokeObjectURL,previousClick=HTMLAnchorElement.prototype.click;
    let request,blob,download;
    window.fetch=async url=>{request=String(url);return {ok:true,json:async()=>({contentType:'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet',filename:'latency.xlsx',contentBase64:btoa('PK\x03\x04')})};};
    URL.createObjectURL=value=>{blob=value;return 'blob:report-test';};URL.revokeObjectURL=()=>{};
    HTMLAnchorElement.prototype.click=function(){download=this.download;};
    const app=Vue.createApp(moqui.qvtComponents['m-report-panel'],{dashboardId:'dashboard',panelId:'latency',range:{from:'2026-01-01T00:00:00Z',thru:'2026-01-02T00:00:00Z'},variables:{service:'ERP'}});
    try {
        const panel=app.mount(target);await panel.exportPanel('xlsx');
        const query=new URL(request,window.location.href).searchParams;
        assert.equal(query.get('from'),'2026-01-01T00:00:00Z');assert.equal(query.get('panelId'),'latency');
        assert.deepEqual(JSON.parse(query.get('variables')),{service:'ERP'});
        assert.deepEqual(Array.from(new Uint8Array(await blob.arrayBuffer())),[80,75,3,4]);assert.equal(download,'latency.xlsx');
    } finally {app.unmount();target.remove();window.fetch=previousFetch;URL.createObjectURL=previousCreate;URL.revokeObjectURL=previousRevoke;HTMLAnchorElement.prototype.click=previousClick;}
});
test("playlist controls move between accessible dashboards and pause rotation", async () => {
    const target=document.createElement('div');document.body.appendChild(target);
    const app=Vue.createApp(moqui.qvtComponents['m-report-playlist'],{items:[{title:'First',url:'/first'},{title:'Second',url:'/second'}],seconds:3600});
    const playlist=app.mount(target);
    assert.equal(target.querySelector('iframe').title,'First');
    Array.from(target.querySelectorAll('button')).find(button=>button.textContent==='Next').click();await Vue.nextTick();
    assert.equal(target.querySelector('iframe').title,'Second');
    Array.from(target.querySelectorAll('button')).find(button=>button.textContent==='Pause').click();await Vue.nextTick();
    assert.equal(playlist.paused,true);
    app.unmount();target.remove();
});
test("structural pills retain typed values, negation, disable, labels and user-scoped pins", async () => {
    const target=document.createElement('div');document.body.appendChild(target);
    const form={fields:Vue.reactive({queryString:'',filtersJson:'[]'})},props={datasetId:'pill-test',viewerId:'reader-test',fields:[{name:'duration',type:'number',searchable:true},{name:'status',type:'string',searchable:true}]};
    localStorage.removeItem(report.storageKey('reader-test','pill-test')+'.pins');
    const app=Vue.createApp(moqui.qvtComponents['m-report-querybar'],props);app.provide('mForm',form);const editor=app.mount(target);
    editor.field='duration';editor.operation='range';editor.value='1,5';editor.label='Expected duration';editor.add();await Vue.nextTick();
    let pill=JSON.parse(form.fields.filtersJson)[0];assert.deepEqual(pill.query,{field:{name:'duration',op:'range',gte:1,lte:5}});
    editor.change(0,'pinned');editor.change(0,'negated');editor.change(0,'disabled');await Vue.nextTick();
    pill=JSON.parse(form.fields.filtersJson)[0];assert.equal(pill.pinned,true);assert.equal(pill.negated,true);assert.equal(pill.disabled,true);
    assert.equal(target.textContent.includes('NOT Expected duration (disabled)'),true);
    editor.edit(0);editor.editJson=JSON.stringify({field:{name:'status',op:'eq',value:'ERROR'}});editor.label='Failures';editor.saveEdit();
    assert.equal(JSON.parse(form.fields.filtersJson)[0].label,'Failures');
    app.unmount();target.remove();
    const next=document.createElement('div');document.body.appendChild(next);const nextForm={fields:Vue.reactive({queryString:'',filtersJson:'[]'})};
    const nextApp=Vue.createApp(moqui.qvtComponents['m-report-querybar'],props);nextApp.provide('mForm',nextForm);const restored=nextApp.mount(next);
    assert.equal(JSON.parse(nextForm.fields.filtersJson)[0].label,'Failures');restored.remove(0);assert.deepEqual(JSON.parse(nextForm.fields.filtersJson),[]);
    nextApp.unmount();next.remove();localStorage.removeItem(report.storageKey('reader-test','pill-test')+'.pins');
});
test("query history and autocomplete reuse authorized fields and sampled values", async () => {
    assert.deepEqual(report.suggestions('severity_text:E',[{name:'severity_text',type:'string'}],{severity_text:['ERROR','WARN']}).map(item=>item.value),['severity_text:"ERROR"']);
    assert.equal(report.replaceQueryToken('name:ERP AND severity_text:E','severity_text:"ERROR"'),'name:ERP AND severity_text:"ERROR"');
    const target=document.createElement('div');document.body.appendChild(target);const form={fields:Vue.reactive({queryString:'severity_text:ERROR',filtersJson:'[]'})};
    const app=Vue.createApp(moqui.qvtComponents['m-report-querybar'],{datasetId:'history-test',viewerId:'history-reader',fields:[{name:'severity_text',type:'string'}]});app.provide('mForm',form);const editor=app.mount(target);
    assert.equal(editor.history[0],'severity_text:ERROR');editor.query='sev';editor.suggesting=true;await Vue.nextTick();
    const suggestion=target.querySelector('[role="listbox"] button');assert.equal(suggestion.textContent.includes('severity_text'),true);suggestion.click();await Vue.nextTick();
    assert.equal(form.fields.queryString,'severity_text:');editor.query='name:ERP';editor.remember();assert.equal(editor.history[0],'name:ERP');
    assert.deepEqual(report.storage.read(report.storageKey('other-user','history-test')+'.history',[]),[]);
    app.unmount();target.remove();localStorage.removeItem(report.storageKey('history-reader','history-test')+'.history');
});
test("real Chart.js panels share timestamps across different bucket widths and synchronize brushed ranges", () => {
    const frame=times=>({fields:[{name:'time',type:'time',values:times},{name:'requests',type:'number',values:times.map(()=>1)}]});
    const first=document.createElement('canvas'),second=document.createElement('canvas');document.body.append(first,second);
    const a=new Chart(first,report.chart(frame([1000,2000,3000]),{syncGroup:'chart-test'})),b=new Chart(second,report.chart(frame([1000,1500,2000,2500,3000]),{syncGroup:'chart-test'}));
    const plugin=a.config.plugins.find(plugin=>plugin.id==='moquiReportSync'),range=[];const onRange=event=>range.push(event.detail);document.addEventListener('moqui-report-range',onRange);
    try {
        plugin.afterEvent(a,{event:{type:'mousemove',x:a.scales.x.getPixelForValue(1),y:a.chartArea.top+5}});
        assert.equal(b.getActiveElements()[0].index,2);
        plugin.afterEvent(a,{event:{type:'mousedown',x:a.scales.x.getPixelForValue(0),y:a.chartArea.top+5}});
        plugin.afterEvent(a,{event:{type:'mouseup',x:a.scales.x.getPixelForValue(1),y:a.chartArea.top+5}});
        assert.equal(a.options.scales.x.max,1);assert.equal(b.options.scales.x.max,2);assert.equal(range[0].from,'1970-01-01T00:00:01.000Z');assert.equal(range[0].thru,'1970-01-01T00:00:02.000Z');
        report.chartSync.reset('chart-test');assert.equal(a.options.scales.x.max,undefined);assert.equal(b.options.scales.x.max,undefined);
        plugin.afterEvent(a,{event:{type:'mouseout'}});assert.equal(b.getActiveElements().length,0);
    } finally {document.removeEventListener('moqui-report-range',onRange);a.destroy();b.destroy();first.remove();second.remove();}
});
test("dashboard keyboard prefixes shift exact ranges and inspect the selected panel", async () => {
    assert.deepEqual(report.shiftRange({from:'2026-01-01T00:00:00Z',thru:'2026-01-02T00:00:00Z'},'ArrowLeft'),{from:'2025-12-31T00:00:00.000Z',thru:'2026-01-01T00:00:00.000Z'});
    assert.deepEqual(report.shiftRange({from:'2026-01-01T00:00:00Z',thru:'2026-01-02T00:00:00Z'},'-'),{from:'2025-12-31T12:00:00.000Z',thru:'2026-01-02T12:00:00.000Z'});
    const target=document.createElement('div');document.body.appendChild(target);
    const app=Vue.createApp(moqui.qvtComponents['m-dashboard'],{dashboard:{panels:{first:{title:'Keyboard panel',visualization:'table'}}},frames:{first:[{fields:[{name:'keyboard-value',type:'number',values:[7]}]}]}});
    const dashboard=app.mount(target);await Vue.nextTick();
    dashboard.onKey({key:'i',target:document.body,preventDefault(){}});await Vue.nextTick();
    assert.equal(target.querySelector('pre').textContent.includes('keyboard-value'),true);
    dashboard.onKey({key:'d',target:document.body,preventDefault(){}});dashboard.onKey({key:'k',target:document.body,preventDefault(){}});await Vue.nextTick();
    assert.equal(dashboard.kiosk,true);
    app.unmount();target.remove();
});
test("retained partial buckets use the typed time axis and exclude metadata series", () => {
    const frame={fields:[{name:'partial',type:'boolean',values:[true,false]},{name:'__level',type:'number',values:[0,0]},{name:'__subtotal',type:'number',values:[1,0]},{name:'time',type:'time',values:[1000,2000]},{name:'requests',type:'number',values:[3,4]},{name:'missing',type:'boolean',values:[true,null]},{name:'other.2',type:'boolean',values:[null,true]}]};
    const config=report.chart(frame,{visualization:'timeSeries'});
    assert.deepEqual(config.data.labels,[report.format(1000,{type:'time'}),report.format(2000,{type:'time'})]);
    assert.deepEqual(config.data.datasets.map(dataset=>dataset.label),['requests']);
    assert.deepEqual(config.data.datasets[0].data,[3,4]);
    const textAxis=report.chart({fields:[frame.fields[0],{name:'category',type:'string',values:['A','B']},frame.fields[4]]},{});
    assert.deepEqual(textAxis.data.labels,['A','B']);
});
function reportBoundComponent(name,value,props) {
    const target=document.createElement('div');document.body.appendChild(target);const model=Vue.reactive({value});let child;
    const app=Vue.createApp({render(){return Vue.h(moqui.qvtComponents[name],Object.assign({},props || {},{value:model.value,onChange:value=>{model.value=value;},ref:value=>{child=value;}}));}});app.mount(target);
    return {target,model,get child(){return child;},close(){app.unmount();target.remove();}};
}
function reportControl(target,label,value) {
    const node=Array.from(target.querySelectorAll('label')).find(node=>node.querySelector('span') && node.querySelector('span').textContent===label);
    if(!node)throw new Error('Missing control '+label);const input=node.querySelector('input,select,textarea');
    if(input.type==='checkbox')input.checked=!!value;else input.value=value;input.dispatchEvent(new Event('change',{bubbles:true}));
}
test('multi-query editing updates the selected query and keeps compound transformations',async()=>{
    const target=document.createElement('div');document.body.appendChild(target);const form={fields:Vue.reactive({definitionJson:JSON.stringify({panels:{one:{title:'Multi',queries:[{datasetId:'otel_logs',mode:'documents'}]}}})})};
    const app=Vue.createApp(moqui.qvtComponents['m-report-definition-editor'],{datasets:[{id:'otel_logs'},{id:'otel_metrics'}]});app.provide('mForm',form);const editor=app.mount(target);
    editor.queries('add');await Vue.nextTick();reportControl(target,'Dataset','otel_metrics');await Vue.nextTick();editor.aggregation(true);await Vue.nextTick();
    let body=JSON.parse(form.fields.definitionJson);assert.equal(body.panels.one.queries.length,2);assert.equal(body.panels.one.queries[0].datasetId,'otel_logs');assert.equal(body.panels.one.queries[1].datasetId,'otel_metrics');assert.equal(body.panels.one.queries[1].aggregations[0].type,'sum');
    editor.update('transformations',[{type:'join',fields:['service']},{type:'calculate',left:'total',right:'count',operator:'/',name:'average'}]);editor.queries('up');await Vue.nextTick();
    body=JSON.parse(form.fields.definitionJson);assert.equal(body.panels.one.queries[0].datasetId,'otel_metrics');assert.deepEqual(body.panels.one.transformations[0].fields,['service']);
    app.unmount();target.remove();
});
test('structured join pivot and organize controls preserve canonical transform arguments',async()=>{
    const view=reportBoundComponent('m-report-transform-editor',[{type:'join',fields:['service']},{type:'pivot',row:'day',column:'region',cell:'amount'},{type:'organize',rename:{'service.name':'Service'}}]);
    let groups=view.target.querySelectorAll('fieldset');reportControl(groups[0],'Key fields (comma separated)','service,region');await Vue.nextTick();groups=view.target.querySelectorAll('fieldset');reportControl(groups[1],'Row totals',true);await Vue.nextTick();groups=view.target.querySelectorAll('fieldset');reportControl(groups[1],'Value field','sales');await Vue.nextTick();
    let organize=Array.from(view.target.querySelectorAll('fieldset')).find(node=>node.querySelector('legend').textContent==='Transformation 3');reportControl(organize,'Rename to','Service title');await Vue.nextTick();organize=Array.from(view.target.querySelectorAll('fieldset')).find(node=>node.querySelector('legend').textContent==='Transformation 3');reportControl(organize,'Hidden fields','internal,count');await Vue.nextTick();
    assert.deepEqual(view.model.value[0].fields,['service','region']);assert.equal(view.model.value[1].cell,'sales');assert.equal(view.model.value[1].rowTotals,true);assert.deepEqual(view.model.value[2].rename,{'service.name':'Service title'});assert.deepEqual(view.model.value[2].hide,['internal','count']);view.close();
});
test('structured field overrides mappings thresholds and data links affect displayed values',async()=>{
    const view=reportBoundComponent('m-report-field-editor',{defaults:{},overrides:[]});
    Array.from(view.target.querySelectorAll('button')).find(button=>button.textContent==='Add field override').click();await Vue.nextTick();
    const override=Array.from(view.target.querySelectorAll('fieldset')).find(node=>node.querySelector('legend').textContent==='Field override 1');reportControl(override,'Match field name','duration');await Vue.nextTick();
    let config=Array.from(view.target.querySelectorAll('fieldset')).find(node=>node.querySelector('legend').textContent==='Override values');reportControl(config,'Field unit','s');await Vue.nextTick();config=Array.from(view.target.querySelectorAll('fieldset')).find(node=>node.querySelector('legend').textContent==='Override values');reportControl(config,'Decimal places','1');await Vue.nextTick();
    config=Array.from(view.target.querySelectorAll('fieldset')).find(node=>node.querySelector('legend').textContent==='Override values');Array.from(config.querySelectorAll('button')).find(button=>button.textContent==='Add threshold').click();await Vue.nextTick();
    config=Array.from(view.target.querySelectorAll('fieldset')).find(node=>node.querySelector('legend').textContent==='Override values');reportControl(config,'Threshold value','2');await Vue.nextTick();config=Array.from(view.target.querySelectorAll('fieldset')).find(node=>node.querySelector('legend').textContent==='Override values');reportControl(config,'Threshold color','negative');await Vue.nextTick();
    config=Array.from(view.target.querySelectorAll('fieldset')).find(node=>node.querySelector('legend').textContent==='Override values');Array.from(config.querySelectorAll('button')).find(button=>button.textContent==='Add value mapping').click();await Vue.nextTick();
    let mapping=Array.from(view.target.querySelectorAll('fieldset')).find(node=>node.querySelector('legend').textContent==='Value mapping 1');reportControl(mapping,'Exact value','0');await Vue.nextTick();mapping=Array.from(view.target.querySelectorAll('fieldset')).find(node=>node.querySelector('legend').textContent==='Value mapping 1');reportControl(mapping,'Display text','Idle');await Vue.nextTick();
    const field=report.configuredFields({fields:[{name:'duration',type:'number',values:[0,3]}]},{fieldConfig:view.model.value})[0];assert.equal(report.format(0,field),'Idle');assert.equal(report.format(3,field),'3.0 s');assert.equal(report.thresholdColor(3,field),'var(--q-negative)');view.close();
});
test('variable and recursive layout editors support ordered options and nested placements',async()=>{
    const vars=reportBoundComponent('m-report-variable-editor',[{name:'region',kind:'custom',options:['EU'],default:'EU'}],{datasets:[{id:'orders'}]});reportControl(vars.target,'Multiple values',true);await Vue.nextTick();reportControl(vars.target,'Default value','EU,US');await Vue.nextTick();reportControl(vars.target,'Include All',true);await Vue.nextTick();assert.deepEqual(vars.model.value[0].default,['EU','US']);assert.equal(vars.model.value[0].includeAll,true);vars.close();
    const layout=reportBoundComponent('m-report-layout-editor',{type:'tabs',items:[{title:'Sales',layout:{type:'grid',items:[{panelId:'sales',x:0,y:0,w:12,h:1}]}}]},{panelIds:['sales','orders']});
    reportControl(layout.target,'Column (0–23)','6');await Vue.nextTick();assert.equal(layout.model.value.items[0].layout.items[0].x,6);
    Array.from(layout.target.querySelectorAll('button')).find(button=>button.textContent==='Add tab').click();await Vue.nextTick();assert.equal(layout.model.value.items.length,2);assert.deepEqual(report.editor.panelIds(layout.model.value),['sales']);layout.close();
});
test('report composition writes ordered dashboard references and per-entry variables',async()=>{
    const target=document.createElement('div');document.body.appendChild(target);const form={fields:Vue.reactive({definitionJson:JSON.stringify({dashboards:[],orientation:'portrait',layout:'grid',appendix:true})})};
    const app=Vue.createApp(moqui.qvtComponents['m-report-composition-editor'],{dashboards:[{id:'sales',title:'Sales'},{id:'orders',title:'Orders'}]});app.provide('mForm',form);const editor=app.mount(target);
    editor.change('dashboards',[{dashboardRef:'sales',from:'now-7d',thru:'now',variables:{region:'EU'}},{dashboardRef:'orders',from:'now-1d',thru:'now',variables:{}}]);await Vue.nextTick();reportControl(target,'Orientation','landscape');await Vue.nextTick();
    const body=JSON.parse(form.fields.definitionJson);assert.deepEqual(body.dashboards.map(entry=>entry.dashboardRef),['sales','orders']);assert.equal(body.dashboards[0].variables.region,'EU');assert.equal(body.orientation,'landscape');assert.equal(target.textContent.includes('Add dashboard to report'),true);app.unmount();target.remove();
});
test('bucket clicks use exact server predicates for normal Missing and Other rows',()=>{
    const filters=[{field:{name:'region',op:'eq',value:'(Missing)'}},{bool:{none:[{field:{name:'region',op:'exists'}}]}},{bool:{all:[{field:{name:'region',op:'exists'}}],none:[{field:{name:'region',op:'in',values:['EU','US','(Missing)']}}]}}];
    const frame={fields:[{name:'region',type:'string',values:['(Missing)',null,'Other'],config:{bucket:{datasetId:'orders',field:'region',kind:'terms',filters,labels:['(Missing)','Missing','Other']}}},{name:'sales',type:'number',values:[5,2,9]}]};
    const clicks=[],config=report.chart(frame,{visualization:'bar'},pill=>clicks.push(pill));config.options.onClick({type:'click'},[{index:1}],{});config.options.onClick({type:'click',shiftKey:true},[{index:2}],{});
    assert.deepEqual(clicks[0].query,filters[1]);assert.equal(clicks[0].negated,false);assert.deepEqual(clicks[1].query,filters[2]);assert.equal(clicks[1].negated,true);assert.equal(report.bucketPill(frame,{},0,0,false).query.field.value,'(Missing)');
    const url=new URL(report.filterUrl(clicks[0],{dashboardId:'sales',filters:[],range:{from:'2026-01-01T00:00:00Z',thru:'2026-01-02T00:00:00Z'}},'https://example.test/apps/dashboards/Dashboard?definitionId=sales&cursor=old'));
    assert.equal(url.searchParams.has('cursor'),false);assert.equal(JSON.parse(url.searchParams.get('filtersJson'))[0].datasetId,'orders');assert.equal(url.searchParams.get('from'),'2026-01-01T00:00:00Z');
});
test('sorting table rows retains the original bucket predicate when clicking values',async()=>{
    const target=document.createElement('div');document.body.appendChild(target);const clicks=[];
    const frame={fields:[{name:'region',type:'string',values:['EU','US'],config:{bucket:{datasetId:'orders',labels:['EU','US'],filters:[{field:{name:'region',op:'eq',value:'EU'}},{field:{name:'region',op:'eq',value:'US'}}]}}},{name:'sales',type:'number',values:[9,1]}]};
    const app=Vue.createApp(moqui.qvtComponents['m-report-panel'],{frame,panel:{visualization:'table'},dashboardId:'sales',onFilter:pill=>clicks.push(pill)});app.mount(target);
    target.querySelectorAll('thead button')[1].click();await Vue.nextTick();target.querySelector('tbody button').click();assert.equal(clicks[0].query.field.value,'US');app.unmount();target.remove();
});
test('split series align time and numeric dimensions with gaps and exact point predicates',()=>{
    const predicates=['EU','EU','US'].map((value,at)=>({bool:{all:[{field:{name:'service',op:'eq',value}},{field:{name:'time',op:'range',gte:at===1?2000:1000,lt:at===1?3000:2000}}]}}));
    const frame={fields:[{name:'time',type:'time',values:[1000,2000,1000]},{name:'service',type:'string',values:['EU','EU','US'],config:{bucket:{datasetId:'requests',filters:predicates,labels:['EU','EU','US']}}},{name:'latency',type:'number',values:[1,2,3]}]};
    const clicks=[],chart=report.chart(frame,{queries:[{aggregations:[{id:'service',role:'series'}]}],fieldConfig:{defaults:{unit:'s',decimals:1}}},pill=>clicks.push(pill));
    assert.equal(chart.data.labels.length,2);assert.equal(chart.data.datasets.length,2);assert.deepEqual(chart.data.datasets[0].data,[1,2]);assert.deepEqual(chart.data.datasets[1].data,[3,null]);
    chart.options.onClick({type:'click'},[{datasetIndex:1,index:0}],{});assert.deepEqual(clicks[0].query,predicates[2]);assert.equal(chart.options.plugins.tooltip.callbacks.label({dataset:chart.data.datasets[0],raw:2}),'latency · service=EU: 2.0 s');
    const numeric=report.chart({fields:[{name:'time',type:'time',values:[1000,1000]},{name:'status',type:'number',values:[200,500],config:{bucket:{datasetId:'requests',filters:[{field:{name:'status',op:'eq',value:200}},{field:{name:'status',op:'eq',value:500}}],labels:['200','500']}}},{name:'count',type:'number',values:[2,1]}]},{queries:[{aggregations:[{id:'status',role:'series'}]}]});
    assert.deepEqual(numeric.data.datasets.map(dataset=>dataset.data),[[2],[1]]);
    const declared=report.chart({fields:[{name:'time',type:'time',values:[1000,1000]},{name:'missing',type:'boolean',values:[true,false]},{name:'count',type:'number',values:[2,1]}]},{queries:[{aggregations:[{id:'missing',role:'series'}]}]});
    assert.equal(declared.data.datasets.length,2);assert.deepEqual(declared.data.datasets.map(dataset=>dataset.data),[[2],[1]]);
});
test('numeric-only aggregate frames retain every metric without inventing an axis field',()=>{
    const config=report.chart({fields:[{name:'count',type:'integer',values:[2]},{name:'total',type:'number',values:[7]}]},{visualization:'bar'});
    assert.deepEqual(config.data.labels,['1']);assert.deepEqual(config.data.datasets.map(dataset=>dataset.label),['count','total']);assert.deepEqual(config.data.datasets.map(dataset=>dataset.data),[[2],[7]]);
});
test('stat and gauge cards resolve integer metrics per numeric bucket series without joining gaps or replacing nulls with zero',async()=>{
    const frame={fields:[{name:'time',type:'time',values:[1000,3000,1000,2000,3000]},{name:'status',type:'number',values:[200,200,500,500,500],config:{bucket:{datasetId:'requests'}}},{name:'requests',type:'integer',unit:'ms',noValue:'Unknown',values:[1,0,3,null,null]},{name:'previous',type:'number',hidden:true,values:[1,0,1,0,0]}]};
    for(const visualization of ['stat','gauge','barGauge']){
        const target=document.createElement('div');document.body.appendChild(target);const app=Vue.createApp(moqui.qvtComponents['m-report-panel'],{frame,panel:{visualization,compareField:'previous',queries:[{aggregations:[{id:'status',role:'series'},{id:'requests',role:'metric'}]}]}});
        app.mount(target);await Vue.nextTick();
        const cards=Array.from(target.querySelectorAll('.m-report-stat-series'));assert.equal(cards.length,2);assert.equal(cards[0].getAttribute('data-report-series'),'requests · status=200');assert.equal(cards[1].getAttribute('data-report-series'),'requests · status=500');
        assert.equal(cards[0].querySelector('.text-h4').textContent,'0 ms');assert.equal(cards[1].querySelector('.text-h4').textContent,'Unknown');assert.equal(target.textContent.includes('Infinity'),false);assert.equal(target.textContent.includes('from previous period'),false);
        if(visualization==='stat'){const path=cards[0].querySelector('path').getAttribute('d');assert.equal((path.match(/M/g) || []).length,2);assert.equal(path.includes('L'),false);}
        else {assert.equal(target.querySelectorAll('meter').length,1);assert.equal(cards[0].querySelector('meter').getAttribute('value'),'0');assert.equal(cards[1].querySelector('meter'),null);}
        app.unmount();target.remove();
    }
    const target=document.createElement('div');document.body.appendChild(target);const app=Vue.createApp(moqui.qvtComponents['m-report-panel'],{frame:{fields:Array.from({length:51},(_,at)=>({name:'metric'+at,type:'integer',values:[at]}))},panel:{visualization:'stat'}});app.mount(target);await Vue.nextTick();assert.equal(target.querySelectorAll('.m-report-stat-series').length,50);assert.equal(target.textContent.includes('Showing 50 of 51 series.'),true);app.unmount();target.remove();
});
test('page links open in the current shell and panel requests use the rendering screen transitions',async()=>{
    assert.equal(report.appPath('/apps/dashboards/Definition?definitionId=x','https://moqui.test/qapps/monitor/dashboard'),'/qapps/dashboards/Definition?definitionId=x');
    assert.equal(report.appPath('/apps/monitor/TraceDetail?traceId=t','https://moqui.test/apps/monitor/Logs'),'/apps/monitor/TraceDetail?traceId=t');
    assert.equal(report.appPath('/apps','https://moqui.test/qapps'),'/qapps');
    assert.equal(report.appPath('/rest/s1/moqui/reports','https://moqui.test/qapps/monitor'),'/rest/s1/moqui/reports');
    assert.equal(report.appPath('/applications/x','https://moqui.test/qapps/monitor'),'/applications/x');
    assert.equal(new URL(report.allPanelsUrl('https://moqui.test/qapps/monitor/dashboard?definitionId=d&panelId=p&var-service=a')).search,'?definitionId=d&var-service=a');
    const target=document.createElement('div');document.body.appendChild(target);
    const previousFetch=window.fetch;const requests=[];
    window.fetch=async url=>{requests.push(String(url));return {ok:true,json:async()=>({dashboard:{panels:{jobs:{inspection:[]}}}})};};
    const app=Vue.createApp(moqui.qvtComponents['m-report-panel'],{dashboardId:'monitor-overview',panelId:'jobs',screenUrl:'/apps/monitor/dashboard',panel:{title:'Job runs by outcome',visualization:'table'},
        frame:{fields:[{name:'outcome',type:'string',values:['success']}]},range:{from:'2026-01-01T00:00:00Z',thru:'2026-01-02T00:00:00Z'}});
    try {
        const panel=app.mount(target);await Vue.nextTick();
        for(const label of ['Export Job runs by outcome as CSV','Export Job runs by outcome as XLSX','Inspect Job runs by outcome'])assert.notEqual(target.querySelector('button[aria-label="'+label+'"]'),null,label);
        await panel.inspect();
        assert.equal(new URL(requests[0],window.location.href).pathname,'/apps/monitor/dashboard/inspectPanel');
        assert.equal(new URL(requests[0],window.location.href).searchParams.get('panelId'),'jobs');
    } finally {app.unmount();target.remove();window.fetch=previousFetch;}
});
test('frozen cells and kiosk take the panel color, kiosk can start open and Esc leaves it',async()=>{
    const target=document.createElement('div');document.body.appendChild(target);
    const app=Vue.createApp(moqui.qvtComponents['m-dashboard'],{dashboard:{kiosk:true,definitionId:'d',panels:{first:{title:'Frozen panel',visualization:'table',fieldConfig:{overrides:[{match:{name:'name'},config:{frozen:true}}]}}}},
        frames:{first:[{fields:[{name:'name',type:'string',values:['row']},{name:'count',type:'number',values:[1]}]}]}});
    try {
        const dashboard=app.mount(target);await Vue.nextTick();
        assert.equal(dashboard.kiosk,true);
        assert.equal(target.querySelector('.m-report-dashboard').getAttribute('style').includes('var(--m-panel-bg'),true);
        assert.equal(Array.from(target.querySelectorAll('td')).find(cell=>cell.textContent==='row').getAttribute('style').includes('var(--m-panel-bg)'),true);
        dashboard.onKey({key:'Escape',target:document.body,preventDefault(){}});await Vue.nextTick();
        assert.equal(dashboard.kiosk,false);
        assert.equal(target.textContent.includes('Kiosk (Alt+K)'),true);
    } finally {app.unmount();target.remove();}
});
test('a single-panel view says so and offers all panels',async()=>{
    const target=document.createElement('div');document.body.appendChild(target);
    const app=Vue.createApp(moqui.qvtComponents['m-dashboard'],{dashboard:{panelId:'first',definitionId:'d',panels:{first:{title:'Only panel',visualization:'stat'}}},frames:{}});
    try {
        app.mount(target);await Vue.nextTick();
        assert.equal(target.textContent.includes('Showing panel Only panel.'),true);
        assert.notEqual(Array.from(target.querySelectorAll('button')).find(button=>button.textContent==='All panels (Esc)'),undefined);
    } finally {app.unmount();target.remove();}
});
test('the waterfall names an ERROR span in its accessible label',async()=>{
    const target=document.createElement('div');document.body.appendChild(target);
    const app=Vue.createApp(moqui.qvtComponents['m-trace-waterfall'],{spans:[{span_id:'a',name:'GET /orders',status_code:'ERROR',start_nano:'1000',duration_nano:2000000},{span_id:'b',parent_span_id:'a',name:'find',status_code:'OK',start_nano:'2000',duration_nano:1000000}]});
    try {
        app.mount(target);await Vue.nextTick();
        const labels=Array.from(target.querySelectorAll('button')).map(button=>button.getAttribute('aria-label'));
        assert.deepEqual(labels,['GET /orders, ERROR, 2.00 milliseconds','find, 1.00 milliseconds']);
    } finally {app.unmount();target.remove();}
});
test('the report composition editor writes the paper size',async()=>{
    const target=document.createElement('div');document.body.appendChild(target);
    const form=Vue.reactive({fields:{definitionJson:JSON.stringify({dashboards:[],orientation:'portrait'})}});
    const app=Vue.createApp({provide:{mForm:form},render:()=>Vue.h(moqui.qvtComponents['m-report-composition-editor'],{dashboards:[]})});
    try {
        app.mount(target);await Vue.nextTick();
        const select=Array.from(target.querySelectorAll('label')).find(label=>label.querySelector('span') && label.querySelector('span').textContent==='Paper').querySelector('select');
        assert.equal(select.value,'letter');
        select.value='a4';select.dispatchEvent(new Event('change'));await Vue.nextTick();
        assert.equal(JSON.parse(form.fields.definitionJson).paper,'a4');
    } finally {app.unmount();target.remove();}
});
function reportButton(target,text){const button=Array.from(target.querySelectorAll('button')).find(node=>node.textContent===text);if(!button)throw new Error('Missing button '+text);return button;}
test('the time picker applies quick, absolute and remembered recent ranges',async()=>{
    const viewer='picker-test-'+Date.now(),key=report.recentRanges.key(viewer);
    const target=document.createElement('div');document.body.appendChild(target);const applied=[];
    const app=Vue.createApp({render(){return Vue.h(moqui.qvtComponents['m-report-time-picker'],{value:{from:'now-7d',thru:'now'},absolute:{from:'2026-01-01T00:00:00.000Z',thru:'2026-01-08T00:00:00.000Z'},viewerId:viewer,onApply:range=>applied.push(range)});}});
    try {
        app.mount(target);await Vue.nextTick();
        const toggle=target.querySelector('button[aria-label^="Time range"]');assert.equal(toggle.textContent,'Last 7 days');
        toggle.click();await Vue.nextTick();
        assert.notEqual(target.querySelector('[role="dialog"]'),null);assert.equal(target.textContent.includes('No recent ranges yet.'),true);
        for(const label of ['Last 5 minutes','Last 30 days','Today','This week','Yesterday','Previous month'])reportButton(target,label);
        reportButton(target,'Last 1 hour').click();await Vue.nextTick();
        assert.deepEqual(applied[0],{from:'now-1h',thru:'now'});assert.equal(target.querySelector('[role="dialog"]'),null);
        assert.deepEqual(report.recentRanges.read(viewer),[{from:'now-1h',thru:'now'}]);
        toggle.click();await Vue.nextTick();
        const recent=Array.from(target.querySelectorAll('h5')).find(node=>node.textContent==='Recent ranges').nextElementSibling;assert.equal(recent.textContent,'Last 1 hour');
        const inputs=target.querySelectorAll('input[type="datetime-local"]');assert.equal(inputs[0].value,report.localInput('2026-01-01T00:00:00.000Z'));
        inputs[0].value='2026-02-02T10:00';inputs[0].dispatchEvent(new Event('input'));inputs[1].value='2026-02-02T09:00';inputs[1].dispatchEvent(new Event('input'));
        reportButton(target,'Apply time range').click();await Vue.nextTick();
        assert.equal(target.querySelector('[role="alert"]').textContent,'The through time must be after the from time');assert.equal(applied.length,1);
        inputs[1].value='2026-02-02T12:30';inputs[1].dispatchEvent(new Event('input'));reportButton(target,'Apply time range').click();await Vue.nextTick();
        assert.deepEqual(applied[1],{from:new Date('2026-02-02T10:00').toISOString(),thru:new Date('2026-02-02T12:30').toISOString()});
        assert.equal(report.recentRanges.read(viewer).length,2);
        reportButton(target,'Earlier').click();await Vue.nextTick();assert.deepEqual(applied[2],{from:'2025-12-25T00:00:00.000Z',thru:'2026-01-01T00:00:00.000Z'});
        assert.equal(report.rangeLabel({from:'now/d',thru:'now/d'}),'Today');assert.equal(report.rangeLabel({from:'now-2h',thru:'now'}),'now-2h to now');
    } finally {app.unmount();target.remove();localStorage.removeItem(key);}
});
test('the dashboard toolbar time picker sends the range to the URL and an embed keeps the page keys and URL',async()=>{
    const target=document.createElement('div');document.body.appendChild(target);const navigate=report.navigate,visited=[];report.navigate=href=>visited.push(href);
    const app=Vue.createApp(moqui.qvtComponents['m-dashboard'],{dashboard:{definitionId:'d',requestedTime:{from:'now-6h',thru:'now'},absoluteTime:{from:'2026-01-01T00:00:00Z',thru:'2026-01-01T06:00:00Z'},panels:{first:{title:'Picked panel',visualization:'stat'}}},frames:{}});
    try {
        const dashboard=app.mount(target);await Vue.nextTick();
        assert.equal(target.querySelector('button[aria-label^="Time range"]').textContent,'Last 6 hours');
        dashboard.timeRange({from:'now-15m',thru:'now'});const url=new URL(visited[0]);assert.equal(url.searchParams.get('from'),'now-15m');assert.equal(url.searchParams.get('thru'),'now');
    } finally {app.unmount();target.remove();report.navigate=navigate;}
    const embedTarget=document.createElement('div');document.body.appendChild(embedTarget);
    const embed=Vue.createApp(moqui.qvtComponents['m-dashboard'],{dashboard:{definitionId:'sales',title:'Sales',embedded:true,editable:true,requestedTime:{from:'now-7d',thru:'now'},absoluteTime:{from:'2026-01-01T00:00:00Z',thru:'2026-01-08T00:00:00Z'},
        panels:{orders:{title:'Orders',visualization:'table',queries:[{datasetId:'orders'}],notices:[{level:'error',message:'Query failed'}]}}},frames:{orders:[{fields:[{name:'count',type:'number',values:[3]}]}]}});
    try {
        const dashboard=embed.mount(embedTarget);await Vue.nextTick();
        assert.equal(dashboard.onKey,undefined);assert.equal(embedTarget.textContent.includes('Kiosk (Alt+K)'),false);
        const open=Array.from(embedTarget.querySelectorAll('a')).find(a=>a.textContent==='Open in Dashboards');assert.equal(new URL(open.href).searchParams.get('definitionId'),'sales');assert.equal(new URL(open.href).searchParams.get('from'),'2026-01-01T00:00:00Z');
        assert.equal(embedTarget.textContent.includes('Orders: Query failed'),true);assert.equal(embedTarget.textContent.includes('Last 7 days'),true);
        embedTarget.querySelector('button[aria-label="Panel menu for Orders"]').click();await Vue.nextTick();
        const items=Array.from(embedTarget.querySelectorAll('[role="menuitem"]')).map(node=>node.textContent);
        assert.deepEqual(items,['Open in Dashboards','Edit','Share','Explore','Inspect']);
    } finally {embed.unmount();embedTarget.remove();}
});
test('the panel menu views, edits, shares, explores, duplicates and removes a panel',async()=>{
    const target=document.createElement('div');document.body.appendChild(target);
    const navigate=report.navigate,request=moqui.request,confirm=window.confirm,visited=[],posts=[];let answer=true;
    report.navigate=href=>visited.push(href);moqui.request=async(url,options)=>{posts.push({url,options});return {ok:true,json:{panelId:'latency-copy'}};};window.confirm=()=>answer;
    const panel={title:'Latency',visualization:'timeSeries',queries:[{datasetId:'otel_metrics',queryString:'name:"http.server.duration"',filters:[{field:{name:'service.name',op:'eq',value:'shop'}}],aggregations:[{id:'time',type:'date_histogram',role:'x-axis'},{id:'p95',type:'max',role:'metric'}]}]};
    const range={from:'2026-01-01T00:00:00Z',thru:'2026-01-02T00:00:00Z'},filters=[{datasetId:'otel_metrics',query:{field:{name:'env',op:'eq',value:'prod'}}},{datasetId:'orders',query:{field:{name:'x',op:'exists'}}}];
    const app=Vue.createApp(moqui.qvtComponents['m-report-panel'],{dashboardId:'ops',panelId:'latency',panel,range,filters,editable:true,frame:{fields:[{name:'time',type:'time',values:[1000]},{name:'p95',type:'number',values:[2]}]}});
    try {
        app.mount(target);await Vue.nextTick();
        const toggle=target.querySelector('button[aria-label="Panel menu for Latency"]');assert.equal(toggle.getAttribute('aria-expanded'),'false');toggle.click();await Vue.nextTick();
        assert.deepEqual(Array.from(target.querySelectorAll('[role="menuitem"]')).map(node=>node.textContent),['View','Edit','Share','Explore','Inspect','Duplicate','Remove','New alert rule']);
        reportButton(target,'View').click();assert.equal(new URL(visited[0]).searchParams.get('panelId'),'latency');
        toggle.click();await Vue.nextTick();reportButton(target,'Edit').click();assert.equal(visited[1],report.appPath('/apps/dashboards/Definition?definitionId=ops&panelId=latency'));
        toggle.click();await Vue.nextTick();reportButton(target,'Explore').click();
        const explore=new URL(visited[2],location.href);assert.equal(explore.pathname.endsWith('/dashboards/Explore'),true);assert.equal(explore.searchParams.get('datasetId'),'otel_metrics');assert.equal(explore.searchParams.get('queryString'),'name:"http.server.duration"');assert.equal(explore.searchParams.get('from'),range.from);
        const pills=JSON.parse(explore.searchParams.get('filtersJson'));assert.equal(pills.length,2);assert.deepEqual(pills[0].query,{field:{name:'service.name',op:'eq',value:'shop'}});assert.equal(pills[1].query.field.name,'env');
        toggle.click();await Vue.nextTick();reportButton(target,'Share').click();for(let wait=0;wait<30 && !target.querySelector('[role="status"]');wait++)await new Promise(resolve=>setTimeout(resolve,50));
        const status=target.querySelector('[role="status"]');assert.notEqual(status,null);const link=new URL(status.querySelector('input')?status.querySelector('input').value:report.shareUrl(range,'latency'));
        assert.equal(link.searchParams.get('from'),range.from);assert.equal(link.searchParams.get('thru'),range.thru);assert.equal(link.searchParams.get('panelId'),'latency');
        toggle.click();await Vue.nextTick();reportButton(target,'Duplicate').click();await new Promise(resolve=>setTimeout(resolve,0));
        assert.equal(posts[0].url,'/apps/dashboards/Dashboard/duplicatePanel');assert.equal(posts[0].options.method,'POST');assert.deepEqual([posts[0].options.params.definitionId,posts[0].options.params.panelId],['ops','latency']);assert.equal(visited.length,4);
        answer=false;toggle.click();await Vue.nextTick();reportButton(target,'Remove').click();await new Promise(resolve=>setTimeout(resolve,0));assert.equal(posts.length,1);
        answer=true;toggle.click();await Vue.nextTick();reportButton(target,'Remove').click();await new Promise(resolve=>setTimeout(resolve,0));assert.equal(posts[1].url,'/apps/dashboards/Dashboard/removePanel');
        moqui.request=async()=>({ok:true,json:{errors:'viewer may not update this dashboard'}});toggle.click();await Vue.nextTick();reportButton(target,'Duplicate').click();await new Promise(resolve=>setTimeout(resolve,0));await Vue.nextTick();
        assert.equal(target.querySelector('[role="alert"]').textContent,'viewer may not update this dashboard');
    } finally {app.unmount();target.remove();report.navigate=navigate;moqui.request=request;window.confirm=confirm;}
    const readOnly=document.createElement('div');document.body.appendChild(readOnly);
    const view=Vue.createApp(moqui.qvtComponents['m-report-panel'],{dashboardId:'ops',panelId:'latency',single:true,panel:{title:'Notes',visualization:'text',text:'x'}});
    try {view.mount(readOnly);await Vue.nextTick();readOnly.querySelector('button[aria-label="Panel menu for Notes"]').click();await Vue.nextTick();
        assert.deepEqual(Array.from(readOnly.querySelectorAll('[role="menuitem"]')).map(node=>node.textContent),['Edit','Share','Inspect']);}
    finally {view.unmount();readOnly.remove();}
});
test('visualization suggestions rank types from the frame columns and the editor applies one',async()=>{
    const first=frame=>report.suggestVisualizations(report.frameShape(frame))[0].visualization;
    assert.equal(first({fields:[{name:'count',type:'number',values:[3]}]}),'stat');
    assert.equal(first({fields:[{name:'time',type:'time',values:[1,2]},{name:'requests',type:'number',values:[3,4]}]}),'timeSeries');
    assert.equal(first({fields:[{name:'status',type:'string',values:['a','b']},{name:'orders',type:'number',values:[3,4]}]}),'bar');
    assert.equal(first({fields:[{name:'time',type:'time',values:[1]},{name:'latency.bucket.0',type:'number',values:[1]},{name:'latency.bucket.1',type:'number',values:[2]}]}),'heatmap');
    assert.equal(first({fields:[{name:'span_id',type:'string',values:['a']},{name:'duration_nano',type:'number',values:[1]}]}),'traceWaterfall');
    const pie=report.suggestVisualizations(report.frameShape({fields:[{name:'status',type:'string',values:['a','b']},{name:'orders',type:'number',values:[3,4]}]})).map(item=>item.visualization);assert.equal(pie.includes('pie'),true);
    assert.equal(report.suggestVisualizations(report.queryShape({queries:[{aggregations:[{id:'total',type:'sum',role:'metric'}]}]}))[0].visualization,'stat');
    assert.equal(report.suggestVisualizations(report.queryShape({queries:[{aggregations:[{id:'time',type:'date_histogram',role:'x-axis'},{id:'n',type:'value_count',role:'metric'}]}]}))[0].visualization,'timeSeries');
    const target=document.createElement('div');document.body.appendChild(target);const previousFetch=window.fetch;let sent;
    window.fetch=async(url,init)=>{sent={url,body:JSON.parse(init.body)};return {ok:true,status:200,json:async()=>({frames:[{fields:[{name:'total',type:'number',values:[9]}]}]})};};
    const form={fields:Vue.reactive({definitionJson:JSON.stringify({panels:{one:{title:'Orders',visualization:'table',queries:[{datasetId:'orders',aggregations:[{id:'status',type:'terms',role:'x-axis'},{id:'total',type:'sum',role:'metric'}]}]}}})})};
    const app=Vue.createApp(moqui.qvtComponents['m-report-definition-editor'],{datasets:[{id:'orders'}]});app.provide('mForm',form);const editor=app.mount(target);
    try {
        await Vue.nextTick();let group=target.querySelector('[aria-label="Suggested visualizations"]');assert.equal(group.textContent.startsWith('Suggested from the query:'),true);
        const best=group.querySelector('button[data-visualization]');assert.equal(best.getAttribute('data-visualization'),'bar');best.click();await Vue.nextTick();
        assert.equal(JSON.parse(form.fields.definitionJson).panels.one.visualization,'bar');
        await editor.preview();await Vue.nextTick();assert.equal(sent.url,'/rest/s1/moqui/reports/query');assert.equal(sent.body.query.queries[0].datasetId,'orders');
        group=target.querySelector('[aria-label="Suggested visualizations"]');assert.equal(group.textContent.startsWith('Suggested from the data:'),true);assert.equal(group.querySelector('button[data-visualization]').getAttribute('data-visualization'),'stat');
        window.fetch=async()=>({ok:false,status:403,json:async()=>({errors:'Not allowed'})});await editor.preview();await Vue.nextTick();assert.equal(target.querySelector('.m-report-suggestions [role="alert"]').textContent,'The data could not be checked: Not allowed');
    } finally {app.unmount();target.remove();window.fetch=previousFetch;}
});
test('table footers total all the data and sparkline cells draw their series',async()=>{
    const target=document.createElement('div');document.body.appendChild(target);
    const frame={fields:[{name:'service',type:'string',values:['a','b']},{name:'hits',type:'number',values:[5,3],config:{totals:{sum:100,max:60}}},{name:'errors',type:'number',values:[1,2]},{name:'sparkline',type:'string',values:['[1,2,3]','[3,null,1]']}]};
    const app=Vue.createApp(moqui.qvtComponents['m-report-panel'],{frame,panel:{title:'Services',visualization:'table',footer:{calculations:['sum','max']}}});
    try {
        app.mount(target);await Vue.nextTick();
        const rows=target.querySelectorAll('tfoot tr');assert.equal(rows.length,2);
        assert.deepEqual(Array.from(rows[0].children).map(cell=>cell.textContent),['Total','100','3','']);
        assert.deepEqual(Array.from(rows[1].children).map(cell=>cell.textContent),['Maximum','60','2','']);
        const sparks=target.querySelectorAll('tbody svg.m-report-sparkline');assert.equal(sparks.length,2);assert.equal((sparks[1].querySelector('path').getAttribute('d').match(/M/g) || []).length,2);
        assert.equal(target.querySelector('tbody').textContent.includes('[1,2,3]'),false);
    } finally {app.unmount();target.remove();}
    assert.equal(report.reduce([1,null,3],'mean'),2);assert.equal(report.reduce([1,2,3,4],'p50'),2.5);assert.equal(report.reduce([],'sum'),null);assert.equal(report.reduce([null,4,null],'last'),4);
    assert.equal(report.sparklineCell({name:'trend',cellDisplay:'sparkline'},[1,2]).length,2);assert.equal(report.sparklineCell({name:'sparkline',cellDisplay:'text'},'[1]'),null);assert.equal(report.sparklineValues('[1,"x"]'),null);
    assert.deepEqual(report.footerRows([{name:'n',type:'number',values:[1]}],{totals:{n:7}})[0].values,[7]);
});
test('text panels substitute dashboard variables as escaped text',async()=>{
    assert.equal(report.substituteVariables('Orders for ${customer} in $region, ${missing} and $5',{customer:'**Acme** <b>',region:['EU','US']}),'Orders for \\*\\*Acme\\*\\* \\<b\\> in EU, US, ${missing} and $5');
    assert.equal(report.substituteVariables('[x](${url})',{url:'javascript:alert(1)'}),'[x](javascript\\:alert\\(1\\))');
    assert.equal(report.substituteVariables('From ${__from}',{},{from:'2026-01-01T00:00:00Z'}),'From 2026\\-01\\-01T00\\:00\\:00Z');
    assert.equal(report.substituteVariables('$service',{service:'$__all'}),'All');
    const target=document.createElement('div');document.body.appendChild(target);
    const app=Vue.createApp(moqui.qvtComponents['m-report-panel'],{panel:{title:'Notes',visualization:'text',text:'Region: ${region}'},variables:{region:'EU'}});
    app.component('m-markdown',{props:{text:String},render(){return Vue.h('pre',{class:'markdown-stub'},this.text);}});
    try {app.mount(target);await Vue.nextTick();assert.equal(target.querySelector('.markdown-stub').textContent,'Region: EU');}
    finally {app.unmount();target.remove();}
});
test('a latency heatmap draws histogram bucket counts over time with the query bounds',async()=>{
    const panel={visualization:'heatmap',heatmap:{unit:'s'},queries:[{datasetId:'otel_metrics',aggregations:[{id:'time',type:'date_histogram',role:'x-axis'},{id:'latency',type:'histogram_merge',role:'metric',params:{bounds:[0.1,0.5]}}]}]};
    const frame={fields:[{name:'time',type:'time',values:[2000,1000,2000]},{name:'latency.p50',type:'number',values:[0.1,0.2,0.3]},{name:'latency.bucket.0',type:'number',values:[1,4,2]},{name:'latency.bucket.1',type:'number',values:[0,1,0]},{name:'latency.bucket.2',type:'number',values:[3,0,null]}]};
    const map=report.heatmap(frame,panel);
    assert.deepEqual(map.labels,['≤ 0.10 s','≤ 0.50 s','> 0.50 s']);assert.deepEqual(map.columns.map(column=>column.time),[1000,2000]);
    assert.deepEqual(map.columns.map(column=>column.counts),[[4,1,0],[3,0,3]]);assert.equal(map.max,4);
    assert.equal(report.heatmap({fields:[{name:'value',type:'number',values:[1]}]},{}),null);
    const target=document.createElement('div');document.body.appendChild(target);
    const app=Vue.createApp(moqui.qvtComponents['m-report-panel'],{frame,panel});
    try {
        app.mount(target);await Vue.nextTick();
        const figure=target.querySelector('figure.m-report-heatmap');assert.equal(figure.getAttribute('role'),'img');assert.equal(figure.getAttribute('aria-label').includes('2 times, 3 buckets'),true);
        const labels=Array.from(figure.querySelectorAll('.m-report-heatmap-label')).map(node=>node.textContent);assert.deepEqual(labels,['> 0.50 s','≤ 0.50 s','≤ 0.10 s']);
        assert.equal(figure.querySelectorAll('[data-count]').length,6);assert.equal(figure.querySelector('[data-count="4"]').getAttribute('title').endsWith('≤ 0.10 s: 4'),true);
    } finally {app.unmount();target.remove();}
});
test('the transformation editor accepts calculation lists, modes and the full filter operators',async()=>{
    const view=reportBoundComponent('m-report-transform-editor',[{type:'reduce',calculations:['last','max']},{type:'calculate',mode:'movingAverage',field:'hits',window:3},{type:'filter',field:'hits',operator:'range',from:1,to:5},{type:'filter',match:'all',conditions:[{field:'a',operator:'exists'}]}]);
    try {
        let groups=view.target.querySelectorAll('fieldset');reportControl(groups[0],'Calculations (comma separated)','last,max,p95');await Vue.nextTick();assert.deepEqual(view.model.value[0].calculations,['last','max','p95']);
        groups=view.target.querySelectorAll('fieldset');reportControl(groups[1],'Window','5');await Vue.nextTick();assert.equal(view.model.value[1].window,5);
        groups=view.target.querySelectorAll('fieldset');reportControl(groups[1],'Calculation mode','binary');await Vue.nextTick();groups=view.target.querySelectorAll('fieldset');assert.notEqual(Array.from(groups[1].querySelectorAll('span')).find(node=>node.textContent==='Left field'),undefined);
        groups=view.target.querySelectorAll('fieldset');reportControl(groups[2],'Range through','9');await Vue.nextTick();assert.equal(view.model.value[2].to,9);
        groups=view.target.querySelectorAll('fieldset');reportControl(groups[3],'Match','any');await Vue.nextTick();assert.equal(view.model.value[3].match,'any');assert.equal(view.target.textContent.includes('1 conditions; edit them in the definition JSON.'),true);
        const operators=Array.from(Array.from(view.target.querySelectorAll('fieldset'))[2].querySelectorAll('label')).find(node=>node.querySelector('span').textContent==='Operator').querySelectorAll('option');
        assert.deepEqual(Array.from(operators).map(option=>option.value),['eq','ne','gt','gte','lt','lte','range','regex','exists','missing']);
    } finally {view.close();}
});

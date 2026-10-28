/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
spec({timeout:90})
function monitoringPage() {
    const page=browser.newPage({user:"john.doe"})
    // Commons returns404 for an absent user photo; the avatar deliberately displays initials (GalleryShots uses this contract too).
    page.allowProblems(/response: 404 Not Found .*\/apps\/Commons\/Settings\/Account\/userPhoto\?userId=EX_JOHN_DOE$/)
    return page
}
feature("a shipped dashboard renders through the report engine in the qvt shell", {
    expect() {
        const page = monitoringPage()
        page.goto("/qapps/dashboards/Dashboard?definitionId=monitor-overview&from=now-1d&thru=now").status == 200
        page.waitFor(() => !!document.querySelector(".m-report-dashboard"))
        page.text().includes("Monitor overview")
        page.text().includes("HTTP requests")
        !/Error rendering|Exception in render|Internal Server Error/.test(page.text())
    },
})
feature("shipped definitions explain the copy workflow and expose version history", {
    expect() {
        const page = monitoringPage()
        page.goto("/apps/dashboards/Definition?definitionId=monitor-overview").status == 200
        page.text().includes("Save a copy to edit it")
        page.text().includes("Save as copy")
        page.text().includes("Export with references")
    },
})
feature("the authorized log search is usable without the qvt shell", {
    expect() {
        const page = monitoringPage()
        page.goto("/apps/monitor/Logs?from=now-1h&thru=now").status == 200
        page.text().includes("Queries run with your dataset permissions")
        page.text().includes("Query")
        !/Error rendering|Exception in render|Internal Server Error/.test(page.text())
    },
})
feature("Explore renders the compiler-free query editor and applies a typed filter through its form", {
    expect() {
        const page=monitoringPage()
        page.goto("/qapps/dashboards/Explore?datasetId=otel_logs&from=now-1h&thru=now").status==200
        page.waitFor(()=>!!document.querySelector('input[aria-label="Search query"]'))
        page.evaluate(()=>{
            const input=document.querySelector('input[aria-label="Search query"]'),editor=input.closest('section')
            const field=Array.from(editor.querySelectorAll('label')).find(label=>label.textContent.startsWith('Field ')).querySelector('select')
            field.value='severity_text';field.dispatchEvent(new Event('change',{bubbles:true}))
            const value=Array.from(editor.querySelectorAll('label')).find(label=>label.textContent.startsWith('Value ')).querySelector('input')
            value.value='ERROR';value.dispatchEvent(new Event('input',{bubbles:true}))
            Array.from(editor.querySelectorAll('button')).find(button=>button.textContent==='Add filter').click();return true
        })
        page.waitFor(()=>document.querySelector('[aria-label="Query filters"]').textContent.includes('severity_text'))
        page.evaluate(()=>{
            const editor=document.querySelector('input[aria-label="Search query"]').closest('section')
            Array.from(editor.querySelectorAll('button')).find(button=>button.textContent==='Negate').click()
            Array.from(editor.querySelectorAll('button')).find(button=>button.textContent==='Disable').click();return true
        })
        page.waitFor(()=>document.querySelector('[aria-label="Query filters"]').textContent.includes('(disabled)'))
        page.evaluate(()=>{document.querySelector('input[aria-label="Search query"]').closest('form').requestSubmit();return true})
        page.waitFor(()=>!!document.querySelector('input[aria-label="Search query"]'))
        !/Error rendering|Exception in render|Internal Server Error/.test(page.text())
    },
})
feature("new panel definitions expose searchable options in the runtime-only browser", {
    expect() {
        const page=monitoringPage()
        page.goto("/qapps/dashboards/Definition?definitionType=panel").status==200
        page.waitFor(()=>!!document.querySelector('input[placeholder="Search panel options"]'))
        page.evaluate(()=>{const input=document.querySelector('input[placeholder="Search panel options"]');input.value='Decimal';input.dispatchEvent(new Event('input',{bubbles:true}));return true})
        page.waitFor(()=>Array.from(document.querySelectorAll('label')).some(label=>label.textContent==='Decimal places'))
        !/Error rendering|Exception in render|Internal Server Error/.test(page.text())
    },
})
feature("explicit dashboard navigation records recent dashboards without writing during a view", {
    expect() {
        const page=monitoringPage()
        page.goto("/qapps/dashboards/Home").status==200
        page.waitFor(()=>Array.from(document.querySelectorAll('a,button')).some(link=>link.textContent.trim()==='Monitor overview'))
        page.evaluate(()=>{const link=Array.from(document.querySelectorAll('a,button')).find(link=>link.textContent.trim()==='Monitor overview');link.click();return true})
        page.waitFor(()=>!!document.querySelector('.m-report-dashboard'))
        page.goto("/apps/dashboards/Home").status==200
        const recent=page.text().split('Recent')[1].split('All definitions')[0]
        recent.includes('Monitor overview')
    },
})

// This function is serialized as one browser-side function: it needs no Node, eval, test-only server route or live model.
async function monitoringOtlpFixture(options) {
    const marker='browser-otlp-'+crypto.randomUUID().replace(/-/g,''),now=BigInt(Date.now())*1000000n;
    const traceJson='e1'.repeat(15)+'01',traceProto='e1'.repeat(15)+'02',spanJson='a1'.repeat(7)+'01',spanProto='a1'.repeat(7)+'02';
    const text=value=>new TextEncoder().encode(value),join=(...parts)=>Uint8Array.from(parts.flatMap(part=>Array.from(part)));
    const variable=value=>{let n=BigInt(value),out=[];do{let b=Number(n&127n);n>>=7n;out.push(n?b|128:b);}while(n);return Uint8Array.from(out);};
    const tag=(number,wire)=>variable(number*8+wire),message=(number,body)=>join(tag(number,2),variable(body.length),body),string=(number,value)=>message(number,text(value));
    const integer=(number,value)=>join(tag(number,0),variable(value));
    const fixed=(number,value)=>{const bytes=new Uint8Array(8);new DataView(bytes.buffer).setBigUint64(0,BigInt(value),true);return join(tag(number,1),bytes);};
    const double=(number,value)=>{const bytes=new Uint8Array(8);new DataView(bytes.buffer).setFloat64(0,value,true);return join(tag(number,1),bytes);};
    const hex=value=>Uint8Array.from(value.match(/../g).map(value=>parseInt(value,16)));
    const attribute=(name,value)=>join(string(1,name),message(2,string(1,value)));
    const resource=(instance)=>join(message(1,attribute('service.name',marker)),...(instance?[message(1,attribute('service.instance.id',instance))]:[]));
    const resourceJson=instance=>({attributes:[{key:'service.name',value:{stringValue:marker}},...(instance?[{key:'service.instance.id',value:{stringValue:instance}}]:[])]});
    // The transition shows the new token once, in its message, and returns to Sources.
    const credentials=await moqui.request('/apps/monitor/Sources/createKey',{method:'POST',params:{source:marker,expireDays:1}});
    const shown=credentials.json && (credentials.json.messageInfos || []).map(info=>info.message).find(message=>/token: mqi_/.test(message));
    if(!credentials.ok || !shown || !/\/Sources/.test(credentials.json.screenUrl || ''))throw new Error('The authorized ingest-key transition failed');
    const token=shown.match(/token: (\S+)/)[1],keyId=shown.match(/ingest key (\S+) for/)[1],statuses=[];
    try {
        const post=async(signal,format,body)=>{const response=await fetch('/v1/'+signal,{method:'POST',credentials:'same-origin',headers:{api_key:token,'Content-Type':format==='json'?'application/json':'application/x-protobuf'},body:format==='json'?JSON.stringify(body):body});statuses.push({signal,format,status:response.status});if(response.status!==200)throw new Error(signal+' '+format+' rejected ('+response.status+')');if(format==='json'){const out=await response.json(),partial=out.partialSuccess || {};for(const value of Object.values(partial))if(/^\d+$/.test(String(value)) && Number(value)>0)throw new Error(signal+' '+format+' partially rejected records');}};
        const span={traceId:traceJson,spanId:spanJson,name:marker+'.json',kind:2,startTimeUnixNano:(now-1000000n).toString(),endTimeUnixNano:now.toString()};
        await post('traces','json',{resourceSpans:[{resource:resourceJson('JSON'),scopeSpans:[{spans:[span]}]}]});
        const protoSpan=join(message(1,hex(traceProto)),message(2,hex(spanProto)),string(5,marker+'.protobuf'),integer(6,2),fixed(7,now-1000000n),fixed(8,now));
        await post('traces','protobuf',message(1,join(message(1,resource('PROTO')),message(2,message(2,protoSpan)))));
        // Missing participates in top-value ranking; keep it and the literal label in the selected three.
        const groups=options && options.complex?[['EU',4],['(Missing)',3],['US',2],[null,5]]:[['JSON',1]];
        await post('logs','json',{resourceLogs:groups.map(([instance,count])=>({resource:resourceJson(instance),scopeLogs:[{logRecords:Array.from({length:count},(_,at)=>({timeUnixNano:now.toString(),observedTimeUnixNano:now.toString(),severityNumber:9,severityText:'INFO',body:{stringValue:marker+'.json.'+(instance || 'missing')+'.'+at},traceId:traceJson,spanId:spanJson}))}]}))});
        const protoLog=join(fixed(1,now),integer(2,9),string(3,'INFO'),message(5,string(1,marker+'.protobuf')),message(9,hex(traceProto)),message(10,hex(spanProto)));
        await post('logs','protobuf',message(1,join(message(1,resource('PROTO')),message(2,message(2,protoLog)))));
        await post('metrics','json',{resourceMetrics:[{resource:resourceJson('JSON'),scopeMetrics:[{metrics:[{name:marker+'.json',unit:'1',gauge:{dataPoints:[{timeUnixNano:now.toString(),asDouble:3.5}]}}]}]}]});
        const point=join(fixed(3,now),double(4,7.5)),metric=join(string(1,marker+'.protobuf'),string(3,'1'),message(5,message(1,point)));
        await post('metrics','protobuf',message(1,join(message(1,resource('PROTO')),message(2,message(2,metric)))));
        const hits={};
        for(let attempt=0;attempt<30;attempt++){
            for(const dataset of ['otel_spans','otel_logs','otel_metrics']){const result=await moqui.request('/apps/dashboards/Explore/tail',{params:{datasetId:dataset,from:'now-1h',queryString:'service.name:'+JSON.stringify(marker)}});if(!result.ok || !result.json)throw new Error('Authorized '+dataset+' verification failed');hits[dataset]=result.json.hits || [];}
            if(hits.otel_spans.some(hit=>hit.body.name===marker+'.json') && hits.otel_spans.some(hit=>hit.body.name===marker+'.protobuf') && hits.otel_logs.some(hit=>String(hit.body.body).startsWith(marker+'.json')) && hits.otel_logs.some(hit=>hit.body.body===marker+'.protobuf') && hits.otel_metrics.some(hit=>hit.body.name===marker+'.json') && hits.otel_metrics.some(hit=>hit.body.name===marker+'.protobuf'))return {marker,statuses,counts:Object.fromEntries(Object.entries(hits).map(([name,rows])=>[name,rows.length]))};
            await new Promise(resolve=>setTimeout(resolve,250));
        }
        throw new Error('Accepted OTLP records did not appear through authorized document queries');
    } finally {
        const revoked=await moqui.request('/apps/monitor/Sources/revokeKey',{method:'POST',params:{keyId}});
        if(!revoked.ok)throw new Error('Fixture ingest key could not be revoked');
    }
}
feature('the browser posts OTLP JSON and protobuf for all three signals with a source key',{
    expect(){
        const page=monitoringPage()
        page.goto('/qapps/monitor/Sources').status==200
        page.waitFor(()=>!!window.moqui && !!moqui.sessionToken)
        const result=page.evaluate(monitoringOtlpFixture,{complex:false})
        result.statuses.length==6
        result.statuses.every(status=>status.status===200)
        result.counts.otel_spans>=2
        result.counts.otel_logs>=2
        result.counts.otel_metrics>=2
    },
})
async function monitoringBuildDashboard(options) {
    const editor=document.querySelector('.m-report-editor'),form=editor.closest('form');
    const tick=()=>Vue.nextTick();
    const control=async(label,value,within)=>{const parent=within || editor,node=Array.from(parent.querySelectorAll('label')).find(node=>{const caption=node.querySelector('span');return (caption?caption.textContent:Array.from(node.childNodes).filter(child=>child.nodeType===Node.TEXT_NODE).map(child=>child.textContent).join('')).trim()===label;});if(!node)throw new Error('Missing editor control '+label);const input=node.querySelector('input,textarea,select');if(input.type==='checkbox')input.checked=!!value;else input.value=value;input.dispatchEvent(new Event('change',{bubbles:true}));await tick();};
    const button=async(text,within)=>{const node=Array.from((within || editor).querySelectorAll('button')).find(button=>button.textContent.trim()===text);if(!node)throw new Error('Missing editor button '+text);node.click();await tick();};
    const draft=()=>JSON.parse(form.querySelector('textarea[name="definitionJson"]').value);
    await button('Add panel');await control('Panel title','Joined browser documents');await control('Visualization','bar');await control('Dataset','otel_logs');await control('Query string','service.name:'+JSON.stringify(options.marker));await control('Query mode','aggregations');
    await button('Add aggregation');await control('Aggregation 1 ID','instance');await control('Aggregation 1 type','terms');await control('Aggregation 1 role','x-axis');await control('Aggregation 1 field','service.instance.id');await control('Aggregation 1 top values','3');await control('Aggregation 1 Missing label','(Missing)');await control('Aggregation 1 Other bucket',true);await control('Aggregation 1 Other label','Other');
    await button('Add aggregation');await control('Aggregation 2 ID','documents');await control('Aggregation 2 type','value_count');await control('Aggregation 2 field','severity_number');
    await button('Duplicate query');await control('Aggregation 2 ID','second_documents');
    await button('Add transformation');await control('Transformation type','join');await control('Key fields (comma separated)','instance');
    await button('Add transformation');let transforms=Array.from(editor.querySelectorAll('fieldset')).filter(node=>node.querySelector('legend') && /^Transformation /.test(node.querySelector('legend').textContent));await control('Transformation type','organize',transforms[1]);transforms=Array.from(editor.querySelectorAll('fieldset')).filter(node=>node.querySelector('legend') && /^Transformation /.test(node.querySelector('legend').textContent));await control('Hidden fields','count,count.2',transforms[1]);
    const queryPanel=Object.keys(draft().panels)[0];
    await button('Add panel');await control('Panel title','Browser instructions');await control('Visualization','text');await control('Markdown text','Browser-authored dashboard notes');const notesPanel=Object.keys(draft().panels).find(id=>id!==queryPanel);
    await control('Layout','tabs');
    const layoutDetails=Array.from(editor.querySelectorAll('details')).find(node=>node.querySelector('summary') && node.querySelector('summary').textContent==='Nested rows, tabs and panel placements');layoutDetails.open=true;
    let layoutControls=Array.from(layoutDetails.querySelectorAll('label')).filter(node=>node.querySelector('span') && node.querySelector('span').textContent==='Layout type');let select=layoutControls[1].querySelector('select');select.value='grid';select.dispatchEvent(new Event('change',{bubbles:true}));await tick();
    await control('Column (0–23)','0',layoutDetails);await button('Add tab',layoutDetails);
    const varsDetails=Array.from(editor.querySelectorAll('details')).find(node=>node.querySelector('summary') && node.querySelector('summary').textContent==='Dashboard variables');varsDetails.open=true;await button('Add variable',varsDetails);await control('Variable name','service',varsDetails);await control('Dataset','otel_logs',varsDetails);await control('Field','service.name',varsDetails);await control('Default value',options.marker,varsDetails);await button('Add variable option',varsDetails);await control('Option value',options.marker,varsDetails);await control('Option label','Browser fixture',varsDetails);
    const id='browser-dashboard-'+options.marker.slice(-16);for(const [name,value] of [['definitionId',id],['definitionTitle','Browser structured dashboard']]){const input=form.querySelector('input[name="'+name+'"]');if(!input)throw new Error('Missing definition field '+name);input.value=value;input.dispatchEvent(new Event('input',{bubbles:true}));}await tick();
    const body=draft();if(body.panels[queryPanel].queries.length!==2 || body.panels[queryPanel].queries.some(query=>query.aggregations[1].type!=='value_count'))throw new Error('UI did not generate declared-field value_count queries');
    form.requestSubmit();return {id,queryPanel,notesPanel,queries:body.panels[queryPanel].queries.length};
}
async function monitoringClickBucket(options) {
    const parameters=new URL(location.href).searchParams,found=await moqui.request('/apps/dashboards/Dashboard/inspectPanel',{params:{definitionId:options.id,panelId:options.panelId,from:'now-1h',thru:'now',filtersJson:parameters.get('filtersJson') || '[]'}});
    if(!found.ok || !found.json || !found.json.dashboard)throw new Error('Authorized panel inspection failed');
    const frame=found.json.dashboard.panels[options.panelId].frames[0],field=frame.fields.find(field=>field.name==='instance'),bucket=field && field.config && field.config.bucket;
    if(!bucket || !bucket.filters)throw new Error('Rendered bucket predicates are absent');
    const row=options.kind==='missing'?field.values.findIndex(value=>value===null):options.kind==='normal'?field.values.findIndex(value=>value==='(Missing)'):bucket.labels.findIndex(value=>value==='Other');
    if(row<0 || !bucket.filters[row])throw new Error('Expected '+options.kind+' bucket is absent');
    const canvas=document.querySelector('.m-report-panel canvas'),chart=Chart.getChart(canvas),datasetIndex=chart.data.datasets.findIndex(dataset=>dataset._reportRows.some((source,index)=>source===row && dataset.data[index]!=null));if(datasetIndex<0)throw new Error('Expected bucket has no plotted metric');const index=chart.data.datasets[datasetIndex]._reportRows.findIndex((source,index)=>source===row && chart.data.datasets[datasetIndex].data[index]!=null),point=chart.getDatasetMeta(datasetIndex).data[index].getCenterPoint(),rect=canvas.getBoundingClientRect();
    canvas.dispatchEvent(new MouseEvent('click',{bubbles:true,clientX:rect.left+point.x*rect.width/chart.width,clientY:rect.top+point.y*rect.height/chart.height}));
    return {query:bucket.filters[row],datasetId:bucket.datasetId};
}
async function monitoringFilteredCount(options) {
    const query=new URL(location.href).searchParams,params={definitionId:options.id,panelId:options.panelId,from:query.get('from') || 'now-1h',thru:query.get('thru') || 'now',filtersJson:query.get('filtersJson') || '[]'};
    const inspected=await moqui.request('/apps/dashboards/Dashboard/inspectPanel',{params});if(!inspected.ok || !inspected.json || !inspected.json.dashboard)throw new Error('Filtered inspection failed');
    const frame=inspected.json.dashboard.panels[options.panelId].frames[0],field=frame.fields.find(field=>field.name==='documents');if(!field)throw new Error('The rendered count field is absent');
    const total=field.values.reduce((sum,value)=>sum+Number(value || 0),0);
    const output=await moqui.request('/apps/dashboards/Dashboard/exportPanel',{params:Object.assign({},params,{renderMode:'csv'})});if(!output.ok || !output.json)throw new Error('Filtered export failed');
    const csv=atob(output.json.contentBase64),lines=csv.replace(/^\uFEFF/,'').trim().split(/\r?\n/).map(line=>line.split(',').map(value=>value.replace(/^"|"$/g,''))),at=lines[0].indexOf('documents');if(at<0)throw new Error('CSV count column absent');const exported=lines.slice(1).reduce((sum,row)=>sum+Number(row[at] || 0),0);
    return {total,exported,filters:JSON.parse(params.filtersJson)};
}
feature('structured UI saves and renders a joined dashboard and bucket clicks filter the same exported data',{
    expect(){
        const page=monitoringPage()
        page.goto('/qapps/monitor/Sources').status==200
        page.waitFor(()=>!!window.moqui && !!moqui.sessionToken)
        const fixture=page.evaluate(monitoringOtlpFixture,{complex:true})
        page.goto('/qapps/dashboards/Explore?datasetId=otel_logs&from=now-1h&thru=now&queryString='+encodeURIComponent('service.name:'+JSON.stringify(fixture.marker))).status==200
        page.waitFor(()=>!!document.querySelector('[data-report-filter-field="severity_text"]'))
        page.evaluate(()=>{document.querySelector('[data-report-filter-field="severity_text"] button').click();return true})
        page.evaluate(()=>{Array.from(document.querySelector('[data-report-filter-field="severity_text"] [role="menu"]').querySelectorAll('button')).find(button=>button.textContent==='Filter for').click();return true})
        page.waitFor(()=>!!new URL(location.href).searchParams.get('filtersJson') && !!document.querySelector('.m-report-value-filter'))
        page.evaluate(()=>JSON.parse(new URL(location.href).searchParams.get('filtersJson'))[0].query.field.name==='severity_text')
        page.goto('/qapps/dashboards/Definition?definitionType=dashboard').status==200
        page.waitFor(()=>!!document.querySelector('.m-report-editor'))
        const saved=page.evaluate(monitoringBuildDashboard,{marker:fixture.marker})
        saved.queries==2
        page.waitFor((id)=>new URL(location.href).searchParams.get('definitionId')===id && document.body.innerText.includes('Used in'),{arg:saved.id})
        page.goto('/qapps/dashboards/Dashboard?definitionId='+encodeURIComponent(saved.id)+'&from=now-1h&thru=now').status==200
        page.waitFor(()=>!!window.Chart && Array.from(document.querySelectorAll('.m-report-panel canvas')).some(canvas=>!!Chart.getChart(canvas)))
        const clickedMissing=page.evaluate(monitoringClickBucket,{id:saved.id,panelId:saved.queryPanel,kind:'missing'})
        page.waitFor(()=>!!new URL(location.href).searchParams.get('filtersJson') && !!document.querySelector('.m-report-dashboard'))
        const missing=page.evaluate(monitoringFilteredCount,{id:saved.id,panelId:saved.queryPanel})
        missing.total==5
        missing.exported==missing.total
        JSON.stringify(missing.filters[0].query)==JSON.stringify(clickedMissing.query)
        page.goto('/qapps/dashboards/Dashboard?definitionId='+encodeURIComponent(saved.id)+'&from=now-1h&thru=now').status==200
        page.waitFor(()=>!!window.Chart && Array.from(document.querySelectorAll('.m-report-panel canvas')).some(canvas=>!!Chart.getChart(canvas)))
        const clickedOther=page.evaluate(monitoringClickBucket,{id:saved.id,panelId:saved.queryPanel,kind:'other'})
        page.waitFor(()=>!!new URL(location.href).searchParams.get('filtersJson') && !!document.querySelector('.m-report-dashboard'))
        const other=page.evaluate(monitoringFilteredCount,{id:saved.id,panelId:saved.queryPanel})
        other.total==3
        other.exported==other.total
        JSON.stringify(other.filters[0].query)==JSON.stringify(clickedOther.query)
        page.goto('/qapps/dashboards/Dashboard?definitionId='+encodeURIComponent(saved.id)+'&from=now-1h&thru=now').status==200
        page.waitFor(()=>!!window.Chart && Array.from(document.querySelectorAll('.m-report-panel canvas')).some(canvas=>!!Chart.getChart(canvas)))
        const clickedNormal=page.evaluate(monitoringClickBucket,{id:saved.id,panelId:saved.queryPanel,kind:'normal'})
        page.waitFor(()=>!!new URL(location.href).searchParams.get('filtersJson') && !!document.querySelector('.m-report-dashboard'))
        const normal=page.evaluate(monitoringFilteredCount,{id:saved.id,panelId:saved.queryPanel})
        normal.total==3
        normal.exported==normal.total
        JSON.stringify(normal.filters[0].query)==JSON.stringify(clickedNormal.query)
        page.goto('/qapps/dashboards/Definition?definitionType=report').status==200
        page.waitFor(()=>document.body.innerText.includes('Report composition'))
        const composed=page.evaluate(monitoringBuildReport,{dashboardId:saved.id,marker:fixture.marker})
        composed.entries==2
        page.waitFor((id)=>new URL(location.href).searchParams.get('definitionId')===id && document.body.innerText.includes('Used in'),{arg:composed.id})
        const reportPreview=page.evaluate(monitoringReportPreview,{id:composed.id})
        reportPreview.hasDocuments
    },
})
async function monitoringBuildReport(options) {
    const editor=document.querySelector('.m-report-editor'),form=editor.closest('form'),tick=()=>Vue.nextTick();
    const control=async(label,value,within)=>{const node=Array.from((within || editor).querySelectorAll('label')).find(node=>node.querySelector('span') && node.querySelector('span').textContent===label);if(!node)throw new Error('Missing report control '+label);const input=node.querySelector('input,select,textarea');if(input.type==='checkbox')input.checked=!!value;else input.value=value;input.dispatchEvent(new Event('change',{bubbles:true}));await tick();};
    const add=async()=>{Array.from(editor.querySelectorAll('button')).find(button=>button.textContent.trim()==='Add dashboard to report').click();await tick();};
    await control('Orientation','landscape');await control('Report layout','rows');await add();let entry=Array.from(editor.querySelectorAll('fieldset')).find(node=>node.querySelector('legend').textContent==='Dashboard 1');await control('Dashboard',options.dashboardId,entry);await control('From','now-1h',entry);
    entry=Array.from(editor.querySelectorAll('fieldset')).find(node=>node.querySelector('legend').textContent==='Dashboard 1');Array.from(entry.querySelectorAll('button')).find(button=>button.textContent.trim()==='Add dashboard variable').click();await tick();entry=Array.from(editor.querySelectorAll('fieldset')).find(node=>node.querySelector('legend').textContent==='Dashboard 1');await control('Variable name','service',entry);entry=Array.from(editor.querySelectorAll('fieldset')).find(node=>node.querySelector('legend').textContent==='Dashboard 1');await control('Value',options.marker,entry);
    await add();entry=Array.from(editor.querySelectorAll('fieldset')).find(node=>node.querySelector('legend').textContent==='Dashboard 2');await control('Dashboard','monitor-overview',entry);
    const id='browser-report-'+options.marker.slice(-16);for(const [name,value] of [['definitionId',id],['definitionTitle','Browser composed report']]){const input=form.querySelector('input[name="'+name+'"]');input.value=value;input.dispatchEvent(new Event('input',{bubbles:true}));}await tick();const body=JSON.parse(form.querySelector('textarea[name="definitionJson"]').value);form.requestSubmit();return {id,entries:body.dashboards.length};
}
async function monitoringReportPreview(options) {
    const result=await moqui.request('/apps/dashboards/Reports/preview',{params:{definitionId:options.id,renderMode:'csv'}});if(!result.ok)throw new Error('Composed report preview failed');const match=result.text.match(/data:text\/csv;base64,([^"\s<>]+)/);if(!match)throw new Error('Composed CSV download absent');return {hasDocuments:atob(match[1]).includes('documents')};
}
// Monitor Logs includes Explore; its row links and forms resolve against /monitor/Logs, so its transitions must be there.
feature('Monitor Logs follows a trace link and the surrounding entries of a row',{
    expect(){
        const page=monitoringPage()
        page.goto('/qapps/monitor/Sources').status==200
        page.waitFor(()=>!!window.moqui && !!moqui.sessionToken)
        const fixture=page.evaluate(monitoringOtlpFixture,{complex:false})
        const logs='/qapps/monitor/Logs?from=now-1h&thru=now&queryString='+encodeURIComponent('service.name:'+JSON.stringify(fixture.marker))
        page.goto(logs).status==200
        page.waitFor(()=>!!document.querySelector('a[href*="/monitor/TraceDetail"]'))
        page.evaluate(()=>Array.from(document.querySelectorAll('a')).filter(a=>a.textContent.trim()==='Surrounding entries').every(a=>/\/LogContext\?/.test(a.getAttribute('href') || '')))
        page.locator('a[href*="/monitor/TraceDetail"]').first().click()
        page.waitFor(()=>location.pathname.endsWith('/monitor/TraceDetail') && /^[0-9a-f]{32}$/.test(new URL(location.href).searchParams.get('traceId') || ''))
        page.waitFor(()=>document.body.innerText.includes('Logs for this trace'))
        page.goto(logs).status==200
        page.waitFor(()=>Array.from(document.querySelectorAll('a')).some(a=>a.textContent.trim()==='Surrounding entries'))
        page.evaluate(()=>{Array.from(document.querySelectorAll('a')).find(a=>a.textContent.trim()==='Surrounding entries').click();return true})
        page.waitFor(()=>location.pathname.endsWith('/dashboards/LogContext'))
        !/Error rendering|Exception in render|Internal Server Error/.test(page.text())
    },
})
feature('the Monitor overview inspects and exports a panel through its own transitions',{
    expect(){
        const page=monitoringPage()
        page.goto('/qapps/monitor/dashboard?from=now-1h&thru=now').status==200
        page.waitFor(()=>!!document.querySelector('button[aria-label="Inspect Job runs by outcome"]'))
        page.evaluate(()=>{const fetch=window.fetch;window.panelRequests=[];window.fetch=function(url,init){window.panelRequests.push(String(url));return fetch.call(this,url,init);};return true})
        page.locator('button[aria-label="Inspect Job runs by outcome"]').click()
        page.waitFor(()=>Array.from(document.querySelectorAll('.m-report-panel pre')).some(pre=>pre.textContent.includes('"dashboard"')))
        page.locator('button[aria-label="Export Job runs by outcome as CSV"]').click()
        page.waitFor(()=>window.panelRequests.some(url=>url.includes('/exportPanel')) && !document.querySelector('button[aria-label="Export Job runs by outcome as CSV"]').disabled)
        const requests=page.evaluate(()=>window.panelRequests.filter(url=>/\/(inspectPanel|exportPanel)\?/.test(url)))
        requests.length==2
        requests.every(url=>url.startsWith('/apps/monitor/dashboard/'))
        page.evaluate(()=>!document.querySelector('.m-report-panel [role="alert"]'))
    },
})
feature('creating an ingest key shows its token once, in a message',{
    expect(){
        const page=monitoringPage()
        page.goto('/qapps/monitor/Sources').status==200
        page.waitFor(()=>!!document.querySelector('form input[name="source"]'))
        page.evaluate(()=>{for(const [name,value] of [['source','browser-token-check'],['expireDays','1']]){const input=document.querySelector('form input[name="'+name+'"]');input.value=value;input.dispatchEvent(new Event('input',{bubbles:true}));}return true})
        page.getByRole('button',{name:'Create ingest key'}).click()
        page.waitFor(()=>/New ingest key [0-9a-f]+ for browser-token-check\. Copy it now; it is not shown again\. token: mqi_[A-Za-z0-9_-]+/.test(document.body.innerText))
        page.waitFor(()=>location.pathname.endsWith('/monitor/Sources'))
    },
})
feature('the dashboard time form keeps the variables and the single-panel view',{
    expect(){
        const page=monitoringPage()
        page.goto('/apps/monitor/Services?from=now-1h&thru=now&panelId=summary&var-service=browser-variable').status==200
        // The marker is on the page before the submit only, so the wait is for the page the form answers with.
        page.evaluate(()=>{window.timeFormSent=true;const form=document.getElementById('DashboardTime');form.querySelector('input[name="from"]').value='now-2h';form.requestSubmit();return true})
        page.waitFor(()=>!window.timeFormSent && document.readyState==='complete' && !!document.getElementById('DashboardTime') && document.querySelector('#DashboardTime input[name="from"]').value==='now-2h')
        const kept=page.evaluate(()=>({values:JSON.parse(document.querySelector('#DashboardTime input[name="variableValuesJson"]').value),panelId:document.querySelector('#DashboardTime input[name="panelId"]').value}))
        kept.values.service=='browser-variable'
        kept.panelId=='summary'
    },
})
async function monitoringCopyOverview(options) {
    const copied=await moqui.request('/apps/dashboards/Definition/copy',{method:'POST',params:{definitionId:'monitor-overview',copyId:options.id,copyTitle:'Browser panel menu copy',moquiSessionToken:moqui.sessionToken}});
    if(!copied.ok)throw new Error('Copying the shipped overview failed ('+copied.status+')');
    return true;
}
async function monitoringPanelMenu(options) {
    const toggle=document.querySelector('button[aria-label="Panel menu for '+options.title+'"]');if(!toggle)throw new Error('Missing panel menu for '+options.title);
    toggle.click();await Vue.nextTick();
    const panel=toggle.closest('.m-report-panel'),items=()=>Array.from(panel.querySelectorAll('[role="menuitem"]'));
    const item=items().find(node=>node.textContent===options.item);
    if(!item){const names=items().map(node=>node.textContent);toggle.click();await Vue.nextTick();return {items:names};}
    window.monitoringMenuClicked=true;const names=items().map(node=>node.textContent);item.click();await Vue.nextTick();
    return {items:names,clicked:true};
}
async function monitoringDefinitionPanels(options) {
    const found=await moqui.request('/apps/dashboards/Dashboard/inspectPanel',{params:{definitionId:options.id,panelId:options.panelId,from:'now-1h',thru:'now'}});
    return found.ok && found.json && found.json.dashboard ? Object.keys(found.json.dashboard.panels) : [];
}
feature('the dashboard time picker, panel menu and an embed work against the server',{
    expect(){
        const page=monitoringPage()
        page.goto('/qapps/dashboards/Dashboard?definitionId=monitor-overview&from=now-1d&thru=now').status==200
        page.waitFor(()=>!!document.querySelector('button[aria-label="Time range: now-1d to now"]'))
        page.evaluate(()=>{document.querySelector('button[aria-label^="Time range"]').click();return true})
        page.waitFor(()=>!!document.querySelector('.m-report-time-ranges'))
        page.evaluate(()=>{window.monitoringPicked=true;Array.from(document.querySelectorAll('.m-report-time-ranges button')).find(button=>button.textContent==='Last 1 hour').click();return true})
        page.waitFor(()=>!window.monitoringPicked && new URL(location.href).searchParams.get('from')==='now-1h' && !!document.querySelector('button[aria-label="Time range: Last 1 hour"]'))
        const shipped=page.evaluate(monitoringPanelMenu,{title:'Job runs by outcome',item:'none'})
        shipped.items.includes('Explore') && shipped.items.includes('Share') && !shipped.items.includes('Remove')
        page.evaluate(monitoringPanelMenu,{title:'Job runs by outcome',item:'Explore'}).clicked
        page.waitFor(()=>!window.monitoringMenuClicked && location.pathname.endsWith('/dashboards/Explore') && !!new URL(location.href).searchParams.get('datasetId'))
        !/Error rendering|Exception in render|Internal Server Error/.test(page.text())
        const id='browser-menu-'+Date.now().toString(36)
        page.evaluate(monitoringCopyOverview,{id})
        page.goto('/qapps/dashboards/Dashboard?definitionId='+id+'&from=now-1h&thru=now').status==200
        page.waitFor(()=>!!document.querySelector('button[aria-label="Panel menu for Job runs by outcome"]'))
        page.evaluate(monitoringPanelMenu,{title:'Job runs by outcome',item:'Duplicate'}).clicked
        page.waitFor(()=>!window.monitoringMenuClicked && !!document.querySelector('button[aria-label="Panel menu for Job runs by outcome (copy)"]'))
        page.evaluate(monitoringDefinitionPanels,{id,panelId:'jobs-copy'}).includes('jobs-copy')
        page.evaluate(()=>{window.confirm=()=>true;return true})
        page.evaluate(monitoringPanelMenu,{title:'Job runs by outcome (copy)',item:'Remove'}).clicked
        page.waitFor(()=>!window.monitoringMenuClicked && !!document.querySelector('button[aria-label="Panel menu for Job runs by outcome"]') && !document.querySelector('button[aria-label="Panel menu for Job runs by outcome (copy)"]'))
        page.goto('/qapps/dashboards/Panels?previewPanelId=monitor-overview').status==200
        page.waitFor(()=>!!document.querySelector('.m-report-embed') && Array.from(document.querySelectorAll('.m-report-embed a')).some(a=>a.textContent==='Open in Dashboards'))
        page.evaluate(()=>!document.querySelector('.m-report-embed button[aria-label^="Time range"]') && document.querySelector('.m-report-embed').textContent.includes('Job runs by outcome'))
        !/Error rendering|Exception in render|Internal Server Error/.test(page.text())
    },
})

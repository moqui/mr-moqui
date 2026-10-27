/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
/* Reporting widgets use server-computed frames. Render functions also run on the compiler-free Vue build. */
(function (root) {
    "use strict";
    const moqui = root.moqui;
    const report = moqui.report = {};
    const finite = (v) => v !== null && v !== undefined && v !== "" && Number.isFinite(Number(v));
    report.fields = (frame) => (frame && Array.isArray(frame.fields) ? frame.fields : []);
    report.numericField = field => ["number", "integer"].includes(field.type || field.kind);
    report.configuredFields = (frame, panel) => report.fields(frame).map((field) => {
        let result = Object.assign({}, panel && panel.fieldConfig && panel.fieldConfig.defaults || {}, field);
        for (const override of panel && panel.fieldConfig && panel.fieldConfig.overrides || []) {
            const match = override.match || {};
            let matches = match.name ? field.name === match.name : match.type ? field.type === match.type : false;
            if (match.regex && String(match.regex).length <= 256) { try { matches = new RegExp(match.regex).test(field.name); } catch (_) { matches = false; } }
            if (matches) result = Object.assign(result, override.config || {});
        }
        return result;
    });
    report.rows = (frame) => {
        const fields = report.fields(frame);
        const count = fields.reduce((n, field) => Math.max(n, (field.values || []).length), 0);
        return Array.from({ length: count }, (_, at) => fields.map((field) => (field.values || [])[at] ?? null));
    };
    report.format = (value, field) => {
        const config = Object.assign({}, field || {}, field && field.config || {});
        if (value === null || value === undefined) return config.noValue || "—";
        for (const mapping of config.mappings || []) {
            if ((mapping.value !== undefined && String(mapping.value) === String(value)) ||
                    (finite(value) && mapping.from !== undefined && Number(value) >= mapping.from && Number(value) <= mapping.to)) return mapping.text;
        }
        if (config.type === "time" || config.kind === "time") {
            const date = new Date(/^\d+$/.test(String(value)) ? Number(value) : value);
            return Number.isNaN(date.getTime()) ? String(value) : date.toLocaleString();
        }
        if (!finite(value) || (config.type && !["number", "integer"].includes(config.type))) return String(value);
        let n = Number(value), suffix = "";
        if (config.unit === "duration" || config.unit === "ns") { n /= 1e6; suffix = " ms"; }
        else if (config.unit === "s") suffix = " s";
        else if (config.unit === "ms") suffix = " ms";
        else if (config.unit === "percent" || config.unit === "%") suffix = "%";
        else if (config.unit === "bytes") {
            const units = [" B", " KiB", " MiB", " GiB", " TiB"];
            let at = 0;
            while (Math.abs(n) >= 1024 && at < units.length - 1) { n /= 1024; at++; }
            suffix = units[at];
        }
        if (/^(currency:)?[A-Z]{3}$/.test(config.unit || "")) {
            const currency = config.unit.replace(/^currency:/, "");
            try { return n.toLocaleString(undefined, { style: "currency", currency, maximumFractionDigits: config.decimals ?? 2 }); } catch (_) { /* unknown unit remains a number */ }
        }
        const decimals = Math.min(12, Math.max(0, config.decimals ?? (Number.isInteger(n) ? 0 : 2)));
        return n.toLocaleString(undefined, { minimumFractionDigits: decimals, maximumFractionDigits: decimals }) + suffix;
    };
    report.csv = (frame) => {
        // Spreadsheet programs interpret these prefixes even in quoted CSV cells.
        const cell = (value, field) => {
            let text = value === null || value === undefined ? "" : String(value);
            const numeric = field && field.type === "number" && /^[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:e[+-]?\d+)?$/i.test(text);
            if (typeof value === "string" && !numeric && /^[=+@\-\t\r]/.test(text)) text = "'" + text;
            return '"' + text.replace(/"/g, '""') + '"';
        };
        const fields = report.fields(frame);
        return [fields.map((field) => cell(field.name)).join(","), ...report.rows(frame).map((row) => row.map((value, at) => cell(value, fields[at])).join(","))].join("\r\n");
    };
    report.color = (at) => {
        const names = ["primary", "positive", "warning", "info", "negative", "secondary", "accent"];
        const name = names[at % names.length];
        return root.getComputedStyle && root.document ? root.getComputedStyle(root.document.documentElement).getPropertyValue("--q-" + name).trim() || name : name;
    };
    report.annotationPositions = (frame, annotations) => {
        const field = report.fields(frame).find((item) => item.type === "time" || ["time", "timestamp", "bucket_start", "start"].includes(item.name));
        if (!field) return [];
        const stamp = (value) => /^\d+$/.test(String(value)) ? Number(value) : Date.parse(value);
        const times = (field.values || []).map(stamp);
        const index = (at) => {
            if (!Number.isFinite(at) || !times.length || at < times[0] || at > times[times.length - 1]) return null;
            let low = 0, high = times.length - 1;
            while (low < high) { const mid = Math.floor((low + high) / 2); if (times[mid] < at) low = mid + 1; else high = mid; }
            return low;
        };
        return (annotations || []).slice(0, 200).map((annotation) => ({ annotation, start: index(stamp(annotation.time)), end: annotation.time_end ? index(stamp(annotation.time_end)) : null }))
            .filter((item) => item.start !== null);
    };
    const syncCharts=new Map();
    report.shiftRange=(range,key)=>{
        const from=Date.parse(range && range.from),thru=Date.parse(range && range.thru),width=thru-from;
        if (!Number.isFinite(width) || width<=0)return null;
        const offsets=key==='-'?[-width/2,width/2]:key==='ArrowLeft'?[-width,-width]:key==='ArrowRight'?[width,width]:null;
        return offsets?{from:new Date(from+offsets[0]).toISOString(),thru:new Date(thru+offsets[1]).toISOString()}:null;
    };
    report.chartSync = {
        group(name){if(!syncCharts.has(name))syncCharts.set(name,new Set());return syncCharts.get(name);},
        clear(name){for(const chart of this.group(name)){chart.$reportCursor=null;chart.setActiveElements([]);if(chart.tooltip)chart.tooltip.setActiveElements([],{x:0,y:0});chart.draw();}},
        zoom(name,from,thru,origin){for(const chart of this.group(name)){const values=chart.$reportValues || [];if(!values.length)continue;const a=report.nearest(values,from),b=report.nearest(values,thru);chart.options.scales.x.min=Math.min(a,b);chart.options.scales.x.max=Math.max(a,b);chart.update('none');}
            if(origin && Number.isFinite(from) && Number.isFinite(thru) && thru>from)root.document.dispatchEvent(new root.CustomEvent('moqui-report-range',{detail:{group:name,from:new Date(from).toISOString(),thru:new Date(thru).toISOString()}}));},
        reset(name){for(const chart of this.group(name)){delete chart.options.scales.x.min;delete chart.options.scales.x.max;chart.update('none');}}
    };
    report.timeValue=value=>typeof value==='number'||/^\d+$/.test(String(value))?Number(value):Date.parse(value);
    report.nearest=(values,value)=>{let best=0,distance=Infinity;values.forEach((item,at)=>{const d=typeof item==='number' && typeof value==='number'?Math.abs(item-value):String(item)===String(value)?0:Infinity;if(d<distance){best=at;distance=d;}});return best;};
    report.syncPlugin=(frame,group)=>{
        const fields=report.fields(frame),field=fields.find(item=>item.type==='time') || fields.find(item=>!report.isMetadata(item) && item.type!=='number') || fields[0];
        const isTime=field && (field.type==='time'||['time','timestamp','bucket_start'].includes(field.name));
        const values=(field && field.values || []).map(value=>isTime?report.timeValue(value):value);
        return {id:'moquiReportSync',afterInit(chart){chart.$reportValues=values;report.chartSync.group(group).add(chart);},afterDestroy(chart){report.chartSync.group(group).delete(chart);if(!report.chartSync.group(group).size)syncCharts.delete(group);},
            afterEvent(chart,args){const event=args.event,scale=chart.scales.x;if(!scale || !values.length)return;const at=Math.max(0,Math.min(values.length-1,Math.round(scale.getValueForPixel(event.x))));
                if(event.type==='mouseout'){report.chartSync.clear(group);return;}
                if(event.type==='mousedown'||event.type==='touchstart'){chart.$reportDrag={at,x:event.x};return;}
                if(event.type==='dblclick'){report.chartSync.reset(group);return;}
                if(event.type==='mouseup'||event.type==='touchend'){const start=chart.$reportDrag;chart.$reportDrag=null;if(isTime && start && Math.abs(start.x-event.x)>4){const from=Math.min(values[start.at],values[at]),thru=Math.max(values[start.at],values[at]);if(thru>from){chart.$reportSuppressClick=true;report.chartSync.zoom(group,from,thru,chart);}}return;}
                if(event.type!=='mousemove'&&event.type!=='touchmove')return;
                for(const other of report.chartSync.group(group)){const index=report.nearest(other.$reportValues || [],values[at]),active=other.data.datasets.map((_,datasetIndex)=>({datasetIndex,index}));other.$reportCursor=index;other.setActiveElements(active);if(other.tooltip)other.tooltip.setActiveElements(active,{x:other.scales.x.getPixelForValue(index),y:other.chartArea.top});if(other!==chart)other.draw();}args.changed=true;
            },afterDraw(chart){if(chart.$reportCursor==null||!chart.scales.x||!chart.chartArea)return;const x=chart.scales.x.getPixelForValue(chart.$reportCursor),area=chart.chartArea;if(x<area.left||x>area.right)return;const ctx=chart.ctx;ctx.save();ctx.strokeStyle=report.color(3);ctx.setLineDash([4,3]);ctx.beginPath();ctx.moveTo(x,area.top);ctx.lineTo(x,area.bottom);ctx.stroke();ctx.restore();}
        };
    };

    report.isMetadata=(field,role)=>["partial","__subtotal","__level"].includes(field.name) || !role && !field.role && !field.config?.role && !field.config?.bucket && (field.type || field.kind)==='boolean' && /^(?:missing|other)(?:\.\d+)?$/.test(field.name);
    report.cellText=(value,field,row)=>{const bucket=field && field.config && field.config.bucket;return bucket && bucket.labels && bucket.labels[row]!=null?String(bucket.labels[row]):report.format(value,field);};
    report.bucketPill=(frame,panel,fieldIndex,rowIndex,negated)=>{
        const field=report.fields(frame)[fieldIndex];if(!field || report.isMetadata(field))return null;
        const bucket=field.config && field.config.bucket;
        if(bucket && bucket.filters && bucket.filters[rowIndex])return {datasetId:bucket.datasetId,query:JSON.parse(JSON.stringify(bucket.filters[rowIndex])),label:(bucket.labels && bucket.labels[rowIndex]) || field.name+': '+report.format((field.values || [])[rowIndex],field),negated:!!negated,disabled:false,pinned:false};
        const queries=panel && panel.queries || [],query=queries.length===1?queries[0]:null,value=(field.values || [])[rowIndex];
        if(!query || query.mode!=='documents' || value && typeof value==='object')return null;
        const name=field.name==='_type'?'_document':field.name;
        return {datasetId:query.datasetId,query:{field:{name,op:value==null?'exists':'eq',...(value==null?{}:{value})}},label:field.name+': '+report.format(value,field),negated:value==null?!negated:!!negated,disabled:false,pinned:false};
    };
    // The shell the page is in (/qapps or /apps). Pages open in it; transition requests keep the server's /apps path.
    report.appRoot=href=>/^\/qapps(?=\/|$)/.test(new root.URL(href || root.location.href).pathname)?'/qapps':'/apps';
    report.appPath=(path,href)=>/^\/apps(?=[/?]|$)/.test(String(path || ''))?report.appRoot(href)+String(path).slice(5):String(path || '');
    // The dashboard of a single-panel view (panelId) with all its panels.
    report.allPanelsUrl=href=>{const url=new root.URL(href || root.location.href);url.searchParams.delete('panelId');return url.href;};
    report.filterUrl=(pill,options,href)=>{const url=new root.URL(href || root.location.href),pills=report.parsePills(options && options.filters || url.searchParams.get('filtersJson'));
        if(pills.length>=32)throw new Error('At most 32 filters');
        if(!pills.some(item=>JSON.stringify([item.datasetId,item.query,!!item.negated])===JSON.stringify([pill.datasetId,pill.query,!!pill.negated])))pills.push(pill);
        if(!(options && options.dashboardId)){url.pathname=report.appRoot(url.href)+'/dashboards/Explore';url.searchParams.set('datasetId',pill.datasetId);}
        url.searchParams.set('filtersJson',JSON.stringify(pills));url.searchParams.delete('cursor');url.searchParams.delete('pageIndex');
        if(options && options.range){for(const key of ['from','thru'])if(options.range[key])url.searchParams.set(key,options.range[key]);}
        return url.href;
    };
    report.aggregationRoles=panel=>{const roles={};const visit=query=>{for(const aggregation of query.aggregations || [])roles[aggregation.id]=aggregation.role;for(const child of query.queries || [])visit(child);};for(const query of panel && panel.queries || [])visit(query);return roles;};
    report.seriesIndexes=(fields,panel,x)=>{const roles=report.aggregationRoles(panel),documents=(panel && panel.queries || []).some(query=>query.mode==='documents');return fields.map((field,at)=>({field,at})).filter(({field,at})=>at!==x && !report.isMetadata(field,roles[field.name]) && (['series','split-series','splitSeries','splitRows','splitChart'].includes(field.role || field.config && field.config.role || roles[field.name]) || field.config && field.config.bucket || !documents && !report.numericField(field))).map(item=>item.at);};
    report.chart = (frame, panel, onFilter) => {
        panel=panel || {};
        const fields=report.configuredFields(frame,panel),rows=report.rows(frame),roles=report.aggregationRoles(panel);
        let x=fields.findIndex(field=>(field.type || field.kind)==='time');
        if(x<0)x=fields.findIndex(field=>!report.isMetadata(field,roles[field.name]) && ((field.role || field.config && field.config.role || roles[field.name])==='x-axis' || field.config && field.config.bucket && !['series','split-series','splitSeries','splitRows','splitChart'].includes(roles[field.name])));
        if(x<0)x=fields.findIndex(field=>!report.isMetadata(field,roles[field.name]) && !report.numericField(field));
        const split=report.seriesIndexes(fields,panel,x),numeric=fields.map((field,at)=>({field,at})).filter(({field,at})=>at!==x && !split.includes(at) && !report.isMetadata(field,roles[field.name]) && !field.hidden && report.numericField(field));
        const coordinates=[],coordinateKeys=new Map(),groups=new Map();
        rows.forEach((row,rowIndex)=>{
            const groupKey=JSON.stringify(split.map(at=>row[at])),rawKey=x<0?String(rowIndex):JSON.stringify(row[x]);let group=groups.get(groupKey);
            const hasSeries=split.every(at=>!fields[at].config?.bucket) || split.some(at=>row[at]!=null || fields[at].config && fields[at].config.bucket && fields[at].config.bucket.filters && fields[at].config.bucket.filters[rowIndex]);
            if(!group){group={label:split.map(at=>(fields[at].displayName || fields[at].name)+'='+report.cellText(row[at],fields[at],rowIndex)).join(' · '),points:[],occurrences:new Map()};if(!split.length || hasSeries)groups.set(groupKey,group);}
            const occurrence=group.occurrences.get(rawKey) || 0;group.occurrences.set(rawKey,occurrence+1);const key=split.length?rawKey+'#'+occurrence:String(rowIndex);
            if(!coordinateKeys.has(key)){coordinateKeys.set(key,coordinates.length);coordinates.push({key,value:x<0?rowIndex+1:row[x],label:x<0?String(rowIndex+1):report.cellText(row[x],fields[x],rowIndex)});}
            if(!split.length || hasSeries)group.points.push({key,rowIndex});
        });
        if((fields[x]?.type || fields[x]?.kind)==='time' && split.length)coordinates.sort((a,b)=>report.timeValue(a.value)-report.timeValue(b.value));
        const positions=new Map(coordinates.map((point,at)=>[point.key,at])),datasets=[];
        const visual=panel.visualization || panel.type || 'timeSeries',type=['pie','donut'].includes(visual)?(visual==='donut'?'doughnut':'pie'):visual==='bar'?'bar':'line';
        numeric.forEach(({field,at},metricIndex)=>{for(const group of groups.values()){
            const values=coordinates.map(()=>null),sourceRows=coordinates.map(()=>null);for(const point of group.points){const position=positions.get(point.key);values[position]=finite(rows[point.rowIndex][at])?Number(rows[point.rowIndex][at]):null;sourceRows[position]=point.rowIndex;}
            const index=datasets.length;datasets.push({label:(field.displayName || field.name)+(group.label?' · '+group.label:''),data:values,_reportRows:sourceRows,_reportField:field,borderColor:report.color(index),backgroundColor:['pie','donut'].includes(visual)?coordinates.map((_,at)=>report.color(at)):field.thresholds?values.map(value=>report.thresholdColor(value,field) || report.color(index)):report.color(index),fill:visual==='area',spanGaps:false,tension:0,pointRadius:1,yAxisID:metricIndex>0 && panel.secondAxis?'y1':'y'});
        }});
        const plotFrame={fields:[Object.assign({},fields[x] || {name:'row',type:'number'},{values:coordinates.map(point=>point.value)})]},annotations=report.annotationPositions(plotFrame,panel.annotations),sync=panel.syncGroup && !['pie','donut'].includes(visual)?[report.syncPlugin(plotFrame,panel.syncGroup)]:[];
        return { type, plugins: annotations.length ? [{ id: "moquiAnnotations", afterDraw(chart) {
            const scale = chart.scales.x, area = chart.chartArea, ctx = chart.ctx;
            if (!scale || !area) return;
            ctx.save(); ctx.beginPath(); ctx.rect(area.left, area.top, area.right - area.left, area.bottom - area.top); ctx.clip();
            for (const marker of annotations) {
                const left = scale.getPixelForValue(marker.start), right = marker.end === null ? null : scale.getPixelForValue(marker.end);
                ctx.fillStyle = report.color(2); ctx.strokeStyle = report.color(2);
                if (right !== null) { ctx.globalAlpha = 0.12; ctx.fillRect(left, area.top, Math.max(1, right - left), area.bottom - area.top); }
                ctx.globalAlpha = 0.8; ctx.beginPath(); ctx.moveTo(left, area.top); ctx.lineTo(left, area.bottom); ctx.stroke();
            }
            ctx.restore();
        } },...sync] : sync, data:{labels:coordinates.map(point=>point.label),datasets},options:{animation:false,responsive:true,maintainAspectRatio:false,
            onClick(event,elements,chart){if(event.type && event.type!=='click' && event.type!=='contextmenu')return;if(chart && chart.$reportSuppressClick){chart.$reportSuppressClick=false;return;}if(!onFilter || !elements.length)return;const hit=elements[0],row=datasets[hit.datasetIndex || 0]?._reportRows[hit.index];if(row==null)return;const pills=[x,...split].map(at=>report.bucketPill(frame,panel,at,row,false)).filter(Boolean);if(!pills.length)return;const pill=pills[0],queries=[];for(const item of pills)if(item.datasetId===pill.datasetId && !queries.some(query=>JSON.stringify(query)===JSON.stringify(item.query)))queries.push(item.query);pill.query=queries.length===1?queries[0]:{bool:{all:queries}};pill.negated=!!((event.native || event).shiftKey);onFilter(pill);},
            events:['click','mousemove','mouseout','mousedown','mouseup','touchstart','touchmove','touchend','dblclick'],interaction:{mode:'index',intersect:false},
            plugins:{tooltip:{callbacks:{label:context=>context.dataset.label+': '+report.format(context.raw,context.dataset._reportField || {})}}},
            scales:['pie','donut'].includes(visual)?undefined:{x:{stacked:!!panel.stacked},y:{stacked:!!panel.stacked,ticks:{callback:value=>report.format(value,numeric[0] && numeric[0].field || {})}},...(panel.secondAxis?{y1:{position:'right',grid:{drawOnChartArea:false},ticks:{callback:value=>report.format(value,numeric[1] && numeric[1].field || {})}}}:{})}}};
    };
    report.waterfall = (spans) => {
        spans = (spans || []).map((span) => span.body || span);
        if (!spans.length) return [];
        // Subtract epoch nanoseconds before converting to a plotting coordinate. Decimal strings retain ns precision.
        const ns = (v) => { try { return BigInt(String(v || 0).split(".")[0]); } catch (_) { return 0n; } };
        const start = (span) => span.start_nano !== undefined ? ns(span.start_nano) : ns(span.start) * 1000000n;
        const first = spans.reduce((a, span) => start(span) < a ? start(span) : a, start(spans[0]));
        const last = spans.reduce((a, span) => start(span) + ns(span.duration_nano) > a ? start(span) + ns(span.duration_nano) : a, first);
        const duration = Number(last - first) || 1;
        const byId = new Map(spans.map((span) => [span.span_id, span]));
        const depth = (span) => {
            const seen = new Set([span.span_id]); let n = 0, parent = span.parent_span_id;
            while (parent && byId.has(parent) && !seen.has(parent) && n < 64) { seen.add(parent); n++; parent = byId.get(parent).parent_span_id; }
            return n;
        };
        return spans.slice().sort((a, b) => start(a) < start(b) ? -1 : start(a) > start(b) ? 1 : String(a.span_id).localeCompare(String(b.span_id))).map((span) => ({
            span, depth: depth(span), left: 100 * Number(start(span) - first) / duration,
            width: Math.min(100, Math.max(0.3, 100 * Number(ns(span.duration_nano)) / duration)), duration: Number(ns(span.duration_nano)) / 1e6,
            missingParent: !!span.parent_span_id && !/^0+$/.test(span.parent_span_id) && !byId.has(span.parent_span_id) }));
    };
    report.safeLink = (url) => typeof url === "string" && (url.startsWith("/") && !url.startsWith("//") || /^https?:\/\//i.test(url)) ? url : "";
    report.dataLink = (url, value, row) => report.safeLink(String(url || "").replace(/\$\{([^}]+)\}/g, (_, name) => encodeURIComponent(name === "value" ? value ?? "" : row[name] ?? "")));
    report.thresholdColor = (value, field) => {
        if (!finite(value)) return "";
        let color = "";
        let n = Number(value);
        if (field.thresholdMode === "percent") n = 100 * (n - (field.min ?? 0)) / ((field.max ?? 100) - (field.min ?? 0) || 1);
        for (const threshold of field.thresholds || []) if (threshold.value == null || n >= threshold.value) {
            color = /^(primary|secondary|accent|positive|negative|warning|info)$/.test(threshold.color) ? "var(--q-" + threshold.color + ")" : /^#[0-9a-f]{3,8}$/i.test(threshold.color || "") ? threshold.color : "";
        }
        return color;
    };
    report.sparkline = (values) => {
        const nums = (values || []).filter(finite).map(Number);
        if (!nums.length) return "";
        const min = Math.min(...nums), max = Math.max(...nums), length = values.length;
        let open = false;
        return values.map((value, at) => {
            if (!finite(value)) { open = false; return ""; }
            const point = (100 * at / Math.max(1, length - 1)).toFixed(2) + "," + (max === min ? 12 : 22 - 20 * (Number(value) - min) / (max - min)).toFixed(2);
            const command = open ? "L" : "M"; open = true; return command + point;
        }).join(" ");
    };
    report.mergeLogHits = (previous, incoming, limit) => {
        const byId = new Map();
        for (const hit of [...previous, ...incoming]) byId.set(hit.documentId + ":" + hit.id, hit);
        const hits = Array.from(byId.values());
        const removed = Math.max(0, hits.length - limit);
        return { hits: hits.slice(-limit), removed };
    };
    const components = moqui.qvtComponents = moqui.qvtComponents || {};
    report.parsePills = (value) => {
        try { const items=typeof value==='string'?JSON.parse(value || '[]'):value;return Array.isArray(items)?items.slice(0,32).filter(item=>item && typeof item==='object' && !Array.isArray(item)):[]; } catch (_) {return [];}
    };
    report.storage = {
        read(key,fallback) {try {return JSON.parse(root.localStorage.getItem(key)) ?? fallback;} catch (_) {return fallback;}},
        write(key,value) {try {root.localStorage.setItem(key,JSON.stringify(value));} catch (_) {/* Private browsing may disable storage. */}}
    };
    report.storageKey=(viewer,dataset)=>"moqui.report."+encodeURIComponent(JSON.stringify([String(viewer || "anonymous"),String(dataset || "dataset")]));
    report.suggestions = (query, fields, values) => {
        const token=String(query || '').match(/(?:^|\s)([^\s]*)$/),last=token?token[1]:'',colon=last.indexOf(':');
        if (colon>=0) {
            const name=last.slice(0,colon),prefix=last.slice(colon+1).replace(/^"/,'').toLowerCase();
            return (values && values[name] || []).filter(value=>String(value).toLowerCase().startsWith(prefix)).slice(0,12).map(value=>({text:name+':'+String(value),value:name+':'+JSON.stringify(value)}));
        }
        return [...(fields || []).filter(field=>field.searchable!==false).map(field=>({text:field.name+' ('+(field.type || 'string')+')',value:field.name+':'})),...['AND','OR','NOT'].map(value=>({text:value,value}))]
            .filter(item=>item.value.toLowerCase().startsWith(last.toLowerCase())).slice(0,12);
    };
    report.replaceQueryToken = (query,value) => String(query || '').replace(/(?:^|\s)[^\s]*$/,match=>(match.startsWith(' ')?' ':'')+value);
    components['m-report-querybar'] = {
        inject:{mForm:{default:null}},
        props:{fields:{type:Array,default:()=>[]},values:{type:Object,default:()=>({})},datasetId:String,viewerId:String,filterOnly:Boolean},
        data:()=>({field:'',operation:'eq',value:'',label:'',editAt:-1,editJson:'',error:'',history:[],suggesting:false,choice:0}),
        computed:{
            formFields(){return this.mForm && this.mForm.fields || {};},
            query:{get(){return String(this.formFields.queryString || '');},set(value){this.formFields.queryString=value;}},
            pills(){return report.parsePills(this.formFields.filtersJson);},
            storageKey(){return report.storageKey(this.viewerId,this.datasetId);},
            suggestions(){return report.suggestions(this.query,this.fields,this.values);}
        },
        mounted(){this.restore();this.remember();},
        watch:{storageKey(){this.restore();}},
        methods:{
            restore(){this.history=report.storage.read(this.storageKey+'.history',[]);if(!Array.isArray(this.history))this.history=[];const pinned=report.parsePills(report.storage.read(this.storageKey+'.pins',[]));if(!this.pills.length && (!this.formFields.filtersJson || this.formFields.filtersJson==='[]') && pinned.length)this.write(pinned);},
            write(pills){this.formFields.filtersJson=JSON.stringify(pills.slice(0,32));report.storage.write(this.storageKey+'.pins',pills.filter(pill=>pill.pinned));},
            remember(){if(!this.query.trim())return;this.history=[this.query,...this.history.filter(query=>query!==this.query)].slice(0,20);report.storage.write(this.storageKey+'.history',this.history);},
            change(at,key){const pills=this.pills;if(pills[at]){pills[at][key]=!pills[at][key];this.write(pills);}},
            remove(at){const pills=this.pills;pills.splice(at,1);this.write(pills);},
            typed(value){const field=this.fields.find(field=>field.name===this.field);if(field && field.type==='number'){const number=Number(value);if(!String(value).trim() || !Number.isFinite(number))throw new Error('Enter a number');return number;}if(field && field.type==='boolean'){if(!['true','false'].includes(value))throw new Error('Enter true or false');return value==='true';}return value;},
            add(){try {if(!this.field)throw new Error('Choose a field');if(this.pills.length>=32)throw new Error('At most 32 filters');const field={name:this.field,op:this.operation};
                if(this.operation==='in')field.values=this.value.split(',').map(value=>this.typed(value.trim()));else if(this.operation==='range'){const values=this.value.split(',');if(values.length!==2)throw new Error('Enter lower,upper bounds');if(values[0].trim())field.gte=this.typed(values[0].trim());if(values[1].trim())field.lte=this.typed(values[1].trim());}else if(this.operation!=='exists')field.value=this.typed(this.value);
                this.write([...this.pills,{datasetId:this.datasetId,query:{field},label:this.label || '',disabled:false,negated:false,pinned:false}]);this.error='';this.value='';this.label='';
            }catch(error){this.error=String(error.message || error);}},
            edit(at){this.editAt=at;this.editJson=JSON.stringify(this.pills[at].query || {},null,2);this.label=this.pills[at].label || '';},
            saveEdit(){try {const query=JSON.parse(this.editJson);if(!query || Array.isArray(query) || typeof query!=='object' || Object.keys(query).length!==1)throw new Error('Enter one structured query object');const pills=this.pills;pills[this.editAt].query=query;pills[this.editAt].label=this.label;this.write(pills);this.editAt=-1;this.label='';this.error='';}catch(error){this.error=String(error.message || error);}},
            choose(item){this.query=report.replaceQueryToken(this.query,item.value);this.suggesting=false;this.choice=0;},
            key(event){if(event.key==='ArrowDown'){event.preventDefault();this.suggesting=true;this.choice=(this.choice+1)%Math.max(1,this.suggestions.length);}else if(event.key==='ArrowUp'){event.preventDefault();this.choice=(this.choice+this.suggestions.length-1)%Math.max(1,this.suggestions.length);}else if(event.key==='Escape'){this.suggesting=false;}else if(event.key==='Enter' && this.suggesting && this.suggestions[this.choice]){event.preventDefault();this.choose(this.suggestions[this.choice]);}else if(event.key==='Enter'){this.remember();}}
        },
        render(){const h=root.Vue.h;return h('section',{class:'q-pa-sm',onSubmit:this.remember},[
            this.filterOnly?null:h('label',['Query ',h('input',{type:'text',value:this.query,style:{width:'min(100%,60rem)'},'aria-label':'Search query','aria-autocomplete':'list','aria-expanded':this.suggesting,onInput:event=>{this.query=event.target.value;this.suggesting=true;this.choice=0;},onKeydown:this.key,onFocus:()=>{this.suggesting=true;},onBlur:()=>{root.setTimeout(()=>{this.suggesting=false;},150);this.remember();}})]),
            this.suggesting && this.suggestions.length?h('ul',{role:'listbox',class:'q-pa-xs'},this.suggestions.map((item,at)=>h('li',{role:'option','aria-selected':at===this.choice},[h('button',{type:'button',onClick:()=>this.choose(item)},item.text)]))):null,
            this.filterOnly?null:h('label',['Recent queries ',h('select',{'aria-label':'Query history',value:'',onChange:event=>{this.query=event.target.value;}},[h('option',{value:''},'Choose a query'),...this.history.map(query=>h('option',{value:query},query))])]),
            this.filterOnly?null:h('button',{type:'button',onClick:()=>{this.history=[];report.storage.write(this.storageKey+'.history',[]);}},'Clear query history'),
            h('div',{class:'row q-gutter-sm','aria-label':'Query filters'},this.pills.map((pill,at)=>h('div',{class:'q-pa-xs',style:{border:'1px solid var(--q-secondary)',opacity:pill.disabled?0.6:1}},[
                h('span',(pill.negated?'NOT ':'')+(pill.label || JSON.stringify(pill.query))+(pill.disabled?' (disabled)':'')),
                h('button',{type:'button','aria-pressed':!!pill.pinned,onClick:()=>this.change(at,'pinned')},pill.pinned?'Unpin':'Pin'),
                h('button',{type:'button','aria-pressed':!!pill.negated,onClick:()=>this.change(at,'negated')},'Negate'),
                h('button',{type:'button','aria-pressed':!!pill.disabled,onClick:()=>this.change(at,'disabled')},pill.disabled?'Enable':'Disable'),
                h('button',{type:'button',onClick:()=>this.edit(at)},'Edit query / label'),h('button',{type:'button',onClick:()=>this.remove(at)},'Remove')]))),
            this.editAt>=0?h('div',[h('label',['Filter label ',h('input',{value:this.label,onInput:event=>{this.label=event.target.value;}})]),h('label',['Structured query ',h('textarea',{value:this.editJson,rows:5,cols:70,onInput:event=>{this.editJson=event.target.value;}})]),h('button',{type:'button',onClick:this.saveEdit},'Apply filter edit'),h('button',{type:'button',onClick:()=>{this.editAt=-1;this.label='';}},'Cancel edit')]):null,
            h('details',[h('summary','Add filter'),h('label',['Field ',h('select',{value:this.field,onChange:event=>{this.field=event.target.value;}},[h('option',{value:''},'Choose field'),...this.fields.filter(field=>field.searchable!==false).map(field=>h('option',{value:field.name},field.name))])]),
                h('label',['Operation ',h('select',{value:this.operation,onChange:event=>{this.operation=event.target.value;}},['eq','in','exists','prefix','wildcard','range'].map(value=>h('option',{value},value)))]),
                h('label',['Value ',h('input',{value:this.value,disabled:this.operation==='exists',placeholder:this.operation==='range'?'lower,upper':this.operation==='in'?'value,value':'value',onInput:event=>{this.value=event.target.value;}})]),
                h('label',['Label ',h('input',{value:this.label,onInput:event=>{this.label=event.target.value;}})]),h('button',{type:'button',onClick:this.add},'Add filter')]),
            h('p','Pinned filters and query history are stored in this browser for this user and dataset. Search applies the filters.'),
            this.error?h('p',{role:'alert',class:'text-negative'},this.error):null]);}
    };

    const E=report.editor={
        clone:value=>JSON.parse(JSON.stringify(value)),
        get:(value,path)=>String(path).split('.').reduce((value,key)=>value==null?undefined:value[key],value),
        set(value,path,next){const parts=String(path).split('.');let target=value;for(let at=0;at<parts.length-1;at++){const key=parts[at];target=target[key] || (target[key]=/^\d+$/.test(parts[at+1])?[]:{});}if(next===undefined)delete target[parts[parts.length-1]];else target[parts[parts.length-1]]=next;},
        list:value=>String(value || '').split(',').map(value=>value.trim()).filter(Boolean),
        move(items,at,delta){const to=at+delta;if(to<0 || to>=items.length)return;[items[at],items[to]]=[items[to],items[at]];},
        button(text,click,disabled){return root.Vue.h('button',{type:'button',disabled:!!disabled,onClick:click,class:'q-btn q-pa-xs'},text);},
        control(label,value,change,type,choices,disabled){const h=root.Vue.h;let input;
            if(choices)input=h('select',{value:value??'',disabled:!!disabled,onChange:event=>change(event.target.value)},choices.map(choice=>h('option',{value:typeof choice==='object'?choice.value:choice},typeof choice==='object'?choice.text || choice.value:choice)));
            else if(type==='textarea')input=h('textarea',{value:value??'',rows:3,disabled:!!disabled,onChange:event=>change(event.target.value)});
            else input=h('input',{type:type==='list'?'text':type || 'text',value:type==='list'?(value || []).join(','):value??'',checked:type==='checkbox'?!!value:undefined,disabled:!!disabled,onChange:event=>change(type==='checkbox'?event.target.checked:type==='number'?(event.target.value===''?undefined:Number(event.target.value)):type==='list'?E.list(event.target.value):event.target.value)});
            return h('label',{class:'m-report-editor-control'},[h('span',label),input]);
        },
        listControls(items,at,change,disabled){return [E.button('Move up',()=>{const copy=E.clone(items);E.move(copy,at,-1);change(copy);},disabled || at===0),E.button('Move down',()=>{const copy=E.clone(items);E.move(copy,at,1);change(copy);},disabled || at===items.length-1),E.button('Remove',()=>{const copy=E.clone(items);copy.splice(at,1);change(copy);},disabled)];},
        panelIds(node){if(!node)return[];if(typeof node==='string')return[node];if(node.panelId)return[node.panelId];return (node.items || node.children || []).flatMap(item=>E.panelIds(item.layout || item));},
        layout(type,ids){const items=ids.map(panelId=>({panelId}));return type==='rows'||type==='tabs'?{type,items:[{title:type==='rows'?'Row':'Tab',layout:{type:'autoGrid',items}}]}:{type,items:type==='grid'?items.map((item,at)=>Object.assign(item,{x:at%2*12,y:Math.floor(at/2),w:12,h:1})):items};}
    };
    components['m-report-transform-editor']={
        props:{value:{type:Array,default:()=>[]},disabled:Boolean},emits:['change'],
        methods:{change(at,path,value){const copy=E.clone(this.value);E.set(copy[at],path,value);this.$emit('change',copy);}},
        render(){const h=root.Vue.h,disabled=this.disabled;
            const fields=this.value.map((step,at)=>{const controls=[],put=(label,path,type,choices)=>controls.push(E.control(label,E.get(step,path),value=>this.change(at,path,value),type,choices,disabled));
                put('Transformation type','type','select',['limit','sort','reduce','filter','calculate','organize','join','joinByTime','merge','pivot','partitionByValues','longToWide','wideToLong','timeSeriesToTable','groupBy','runningTotal','movingAverage','delta','percentOfTotal','timeShift']);put('Disabled','disabled','checkbox');
                if(['sort','filter','runningTotal','movingAverage','delta','percentOfTotal','timeShift'].includes(step.type))put('Field','field');
                if(step.type==='limit')put('Row limit','count','number');
                if(step.type==='sort')put('Descending','descending','checkbox');
                if(['reduce','groupBy'].includes(step.type))put('Calculation','calculation','select',['sum','mean','min','max','count','first','last','delta','p50','p95','p99']);
                if(['join','joinByTime','partitionByValues','groupBy'].includes(step.type))put('Key fields (comma separated)','fields','list');
                if(step.type==='filter'){put('Operator','operator','select',['eq','ne','gt','lt','exists']);controls.push(E.control('Value type',step.value===null?'null':typeof step.value,value=>this.change(at,'value',value==='number'?0:value==='boolean'?false:value==='null'?null:''),'select',['string','number','boolean','null'],disabled));if(step.value!==null)put('Value','value',typeof step.value==='number'?'number':typeof step.value==='boolean'?'checkbox':'text');}
                if(step.type==='calculate'){put('Output field','name');put('Left field','left');put('Right field (or use constant)','right');put('Constant','value','number');put('Operator','operator','select',['+','-','*','/']);}
                if(['runningTotal','movingAverage','delta','percentOfTotal'].includes(step.type))put('Output field','name');
                if(step.type==='movingAverage')put('Window','window','number');if(step.type==='timeShift')put('Shift milliseconds','millis','number');
                if(step.type==='pivot'){put('Row field','row');put('Column field','column');put('Value field','cell');put('Row totals','rowTotals','checkbox');put('Column totals','columnTotals','checkbox');}
                if(['longToWide','wideToLong'].includes(step.type))put('Time field','timeField');
                if(step.type==='longToWide'){put('Series field','seriesField');put('Value field','valueField');}
                if(step.type==='organize'){put('Hidden fields','hide','list');put('Field order','order','list');const pairs=Object.entries(step.rename || {});
                    pairs.forEach(([from,to],index)=>controls.push(h('div',{class:'row q-gutter-sm'},[E.control('Original field',from,value=>{const rename=Object.assign({},step.rename);delete rename[from];rename[value]=to;this.change(at,'rename',rename);},null,null,disabled),E.control('Rename to',to,value=>{const rename=Object.assign({},step.rename);rename[from]=value;this.change(at,'rename',rename);},null,null,disabled),E.button('Remove rename',()=>{const rename=Object.assign({},step.rename);delete rename[from];this.change(at,'rename',rename);},disabled)])));
                    controls.push(E.button('Add field rename',()=>{const rename=Object.assign({},step.rename);let name='field';while(name in rename)name+='-new';rename[name]='New name';this.change(at,'rename',rename);},disabled));
                }
                return h('fieldset',{class:'m-report-editor-group',key:at},[h('legend','Transformation '+(at+1)),...controls,...E.listControls(this.value,at,value=>this.$emit('change',value),disabled)]);
            });
            return h('section',[...fields,E.button('Add transformation',()=>this.$emit('change',[...E.clone(this.value),{type:'limit',count:100}]),disabled)]);
        }
    };
    components['m-report-field-editor']={
        props:{value:{type:Object,default:()=>({})},disabled:Boolean},emits:['change'],
        methods:{change(path,value){const copy=E.clone(this.value);E.set(copy,path,value);this.$emit('change',copy);}},
        render(){const h=root.Vue.h,disabled=this.disabled,config=this.value;
            const settings=(value,change,title)=>{const rows=[],put=(label,path,type,choices)=>rows.push(E.control(label,E.get(value,path),next=>{const copy=E.clone(value);E.set(copy,path,next);change(copy);},type,choices,disabled));
                put('Display name','displayName');put('Field unit','unit');put('Decimal places','decimals','number');put('Field minimum','min','number');put('Field maximum','max','number');put('No-value text','noValue');put('Hidden','hidden','checkbox');put('Frozen column','frozen','checkbox');put('Data link URL (${value} or ${field})','link');put('Threshold mode','thresholdMode','select',['absolute','percent']);
                const mappings=value.mappings || [];mappings.forEach((mapping,at)=>rows.push(h('fieldset',{class:'m-report-editor-group'},[h('legend','Value mapping '+(at+1)),E.control('Exact value',mapping.value,next=>{const copy=E.clone(value);copy.mappings[at].value=next;delete copy.mappings[at].from;delete copy.mappings[at].to;change(copy);},null,null,disabled),E.control('Range from',mapping.from,next=>{const copy=E.clone(value);copy.mappings[at].from=next;delete copy.mappings[at].value;change(copy);},'number',null,disabled),E.control('Range through',mapping.to,next=>{const copy=E.clone(value);copy.mappings[at].to=next;delete copy.mappings[at].value;change(copy);},'number',null,disabled),E.control('Display text',mapping.text,next=>{const copy=E.clone(value);copy.mappings[at].text=next;change(copy);},null,null,disabled),...E.listControls(mappings,at,next=>{const copy=E.clone(value);copy.mappings=next;change(copy);},disabled)])));
                rows.push(E.button('Add value mapping',()=>change(Object.assign({},value,{mappings:[...E.clone(mappings),{value:'',text:''}]})),disabled));
                const thresholds=value.thresholds || [];thresholds.forEach((threshold,at)=>rows.push(h('div',{class:'row q-gutter-sm'},[E.control('Threshold value',threshold.value,next=>{const copy=E.clone(value);copy.thresholds[at].value=next;change(copy);},'number',null,disabled),E.control('Threshold color',threshold.color,next=>{const copy=E.clone(value);copy.thresholds[at].color=next;change(copy);},'select',['positive','warning','negative','primary','secondary','info','accent'],disabled),...E.listControls(thresholds,at,next=>{const copy=E.clone(value);copy.thresholds=next;change(copy);},disabled)])));
                rows.push(E.button('Add threshold',()=>change(Object.assign({},value,{thresholds:[...E.clone(thresholds),{value:0,color:'warning'}]})),disabled));
                return h('fieldset',{class:'m-report-editor-group'},[h('legend',title),...rows]);
            };
            const overrides=config.overrides || [];
            return h('section',[settings(config.defaults || {},value=>this.change('defaults',value),'Field defaults'),...overrides.map((override,at)=>h('fieldset',{class:'m-report-editor-group'},[h('legend','Field override '+(at+1)),E.control('Match field name',override.match && override.match.name,value=>this.change('overrides.'+at+'.match',{name:value}),null,null,disabled),E.control('Match regular expression',override.match && override.match.regex,value=>this.change('overrides.'+at+'.match',{regex:value}),null,null,disabled),E.control('Match field type',override.match && override.match.type,value=>this.change('overrides.'+at+'.match',{type:value}),'select',['','number','string','time','boolean'],disabled),settings(override.config || {},value=>this.change('overrides.'+at+'.config',value),'Override values'),...E.listControls(overrides,at,value=>this.change('overrides',value),disabled)])),E.button('Add field override',()=>this.change('overrides',[...E.clone(overrides),{match:{name:''},config:{}}]),disabled)]);
        }
    };
    components['m-report-variable-editor']={
        props:{value:{type:Array,default:()=>[]},datasets:{type:Array,default:()=>[]},disabled:Boolean},emits:['change'],
        methods:{change(at,path,value){const copy=E.clone(this.value);E.set(copy[at],path,value);this.$emit('change',copy);}},
        render(){const h=root.Vue.h,disabled=this.disabled;
            return h('section',[...this.value.map((variable,at)=>{const controls=[],put=(label,path,type,choices)=>controls.push(E.control(label,E.get(variable,path),value=>this.change(at,path,value),type,choices,disabled));
                put('Variable name','name');put('Label','label');put('Kind','kind','select',['query','custom','constant','interval','text','groupBy']);put('Dataset','datasetId','select',[{value:'',text:'All query datasets'},...this.datasets.map(item=>({value:item.id,text:item.title || item.id}))]);put('Field','field');put('Multiple values','multi','checkbox');put('Include All','includeAll','checkbox');
                controls.push(E.control('Default value',Array.isArray(variable.default)?variable.default.join(','):variable.default,value=>this.change(at,'default',variable.multi?E.list(value):value),null,null,disabled));
                const options=variable.options || [];options.forEach((option,index)=>{const item=typeof option==='object'?option:{value:option,text:String(option)};controls.push(h('div',{class:'row q-gutter-sm'},[E.control('Option value',item.value,value=>{const copy=E.clone(options);copy[index]={value,text:item.text};this.change(at,'options',copy);},null,null,disabled),E.control('Option label',item.text,value=>{const copy=E.clone(options);copy[index]={value:item.value,text:value};this.change(at,'options',copy);},null,null,disabled),...E.listControls(options,index,value=>this.change(at,'options',value),disabled)]));});
                controls.push(E.button('Add variable option',()=>this.change(at,'options',[...E.clone(options),{value:'',text:''}]),disabled));
                return h('fieldset',{class:'m-report-editor-group'},[h('legend','Variable '+(at+1)),...controls,...E.listControls(this.value,at,value=>this.$emit('change',value),disabled)]);
            }),E.button('Add variable',()=>this.$emit('change',[...E.clone(this.value),{name:'variable'+(this.value.length+1),kind:'custom',options:[],default:''}]),disabled)]);
        }
    };
    components['m-report-layout-editor']={
        props:{value:{type:Object,default:()=>({type:'autoGrid',items:[]})},panelIds:{type:Array,default:()=>[]},disabled:Boolean},emits:['change'],
        render(){const h=root.Vue.h,disabled=this.disabled;
            const render=(node,change,depth)=>{if(depth>12)return h('p',{role:'alert'},'Layout nesting exceeds 12 levels');const items=node.items || node.children || [],type=node.type || 'autoGrid';
                const setItems=next=>change(Object.assign({},node,{items:next}));
                return h('fieldset',{class:'m-report-editor-group'},[h('legend','Layout level '+(depth+1)),E.control('Layout type',type,value=>change(E.layout(value,E.panelIds(node))), 'select',['autoGrid','grid','rows','tabs'],disabled),
                    ...items.map((item,at)=>{const update=value=>{const next=E.clone(items);next[at]=value;setItems(next);};return h('div',{class:'m-report-editor-layout-item'},[
                        type==='rows'||type==='tabs'?h('div',[E.control(type==='rows'?'Row title':'Tab title',item.title,value=>update(Object.assign({},item,{title:value})),null,null,disabled),type==='rows'?E.control('Initially collapsed',item.collapsed,value=>update(Object.assign({},item,{collapsed:value})),'checkbox',null,disabled):null,render(item.layout || {type:'autoGrid',items:[]},value=>update(Object.assign({},item,{layout:value})),depth+1)]):
                        h('div',[E.control('Panel',typeof item==='string'?item:item.panelId,value=>update(Object.assign({},typeof item==='object'?item:{},{panelId:value})),'select',this.panelIds,disabled),...(type==='grid'?['x','y','w','h'].map(key=>E.control({x:'Column (0–23)',y:'Row',w:'Width (1–24)',h:'Height'}[key],item[key],value=>update(Object.assign({},item,{[key]:value})),'number',null,disabled)):[])]),...E.listControls(items,at,setItems,disabled)]);}),
                    E.button(type==='rows'?'Add row':type==='tabs'?'Add tab':'Add panel placement',()=>setItems([...E.clone(items),type==='rows'||type==='tabs'?{title:type==='rows'?'Row':'Tab',layout:{type:'autoGrid',items:[]}}:{panelId:this.panelIds[0] || '',...(type==='grid'?{x:0,y:0,w:12,h:1}:{})}]),disabled)]);
            };return render(this.value,value=>this.$emit('change',value),0);
        }
    };
    components['m-report-composition-editor']={
        inject:{mForm:{default:null}},props:{name:{type:String,default:'definitionJson'},dashboards:{type:Array,default:()=>[]},readOnly:Boolean},
        computed:{body(){try{return JSON.parse(this.mForm.fields[this.name] || '{}');}catch(_){return null;}}},
        methods:{write(body){if(!this.readOnly)this.mForm.fields[this.name]=JSON.stringify(body,null,2);},change(path,value){const copy=E.clone(this.body);E.set(copy,path,value);this.write(copy);}},
        render(){const h=root.Vue.h,body=this.body,disabled=this.readOnly;if(!body || typeof body!=='object')return h('p',{role:'alert'},'Correct the JSON to use the report editor');const rows=body.dashboards || [];
            if(body.kind==='playlist'){const items=body.playlist || [];return h('section',{class:'m-report-editor'},[h('h4','Playlist options'),E.control('Seconds per dashboard',body.seconds || 30,value=>this.change('seconds',value),'number',null,disabled),...items.map((entry,at)=>h('fieldset',{class:'m-report-editor-group'},[h('legend','Playlist item '+(at+1)),E.control('Dashboard',entry.definitionId,value=>this.change('playlist.'+at+'.definitionId',value),'select',this.dashboards.map(item=>({value:item.id,text:item.title || item.id})),disabled),...E.listControls(items,at,value=>this.change('playlist',value),disabled)])),E.button('Add playlist dashboard',()=>this.change('playlist',[...E.clone(items),{definitionId:this.dashboards[0] && this.dashboards[0].id || ''}]),disabled)]);}
            return h('section',{class:'m-report-editor'},[h('h4','Report composition'),E.control('Orientation',body.orientation || 'portrait',value=>this.change('orientation',value),'select',['portrait','landscape'],disabled),E.control('Paper',body.paper || 'letter',value=>this.change('paper',value),'select',[{value:'letter',text:'Letter'},{value:'a4',text:'A4'}],disabled),E.control('Report layout',body.layout || 'grid',value=>this.change('layout',value),'select',['grid','rows'],disabled),E.control('Appendix',body.appendix,value=>this.change('appendix',value),'checkbox',null,disabled),
                ...rows.map((entry,at)=>h('fieldset',{class:'m-report-editor-group'},[h('legend','Dashboard '+(at+1)),E.control('Dashboard',entry.dashboardRef,value=>this.change('dashboards.'+at+'.dashboardRef',value),'select',this.dashboards.map(item=>({value:item.id,text:item.title || item.id})),disabled),E.control('From',entry.from,value=>this.change('dashboards.'+at+'.from',value),null,null,disabled),E.control('Through',entry.thru,value=>this.change('dashboards.'+at+'.thru',value),null,null,disabled),
                    ...Object.entries(entry.variables || {}).map(([name,value])=>h('div',{class:'row q-gutter-sm'},[E.control('Variable name',name,next=>{const variables=Object.assign({},entry.variables);delete variables[name];variables[next]=value;this.change('dashboards.'+at+'.variables',variables);},null,null,disabled),E.control('Value',Array.isArray(value)?value.join(','):value,next=>{const variables=Object.assign({},entry.variables);variables[name]=next;this.change('dashboards.'+at+'.variables',variables);},null,null,disabled),E.button('Remove variable',()=>{const variables=Object.assign({},entry.variables);delete variables[name];this.change('dashboards.'+at+'.variables',variables);},disabled)])),
                    E.button('Add dashboard variable',()=>{const variables=Object.assign({},entry.variables);let name='variable';while(name in variables)name+='-new';variables[name]='';this.change('dashboards.'+at+'.variables',variables);},disabled),...E.listControls(rows,at,value=>this.change('dashboards',value),disabled)])),
                E.button('Add dashboard to report',()=>this.change('dashboards',[...E.clone(rows),{dashboardRef:this.dashboards[0] && this.dashboards[0].id || '',variables:{},from:'now-7d',thru:'now'}]),disabled)]);
        }
    };

    components['m-report-value-filter']={
        props:{datasetId:String,field:String,value:null,label:String,link:String,filters:{type:Array,default:()=>[]},range:Object,hideValue:Boolean},
        data:()=>({open:false,error:''}),
        methods:{apply(negated){try{const pill={datasetId:this.datasetId,query:{field:{name:this.field,op:this.value==null?'exists':'eq',...(this.value==null?{}:{value:this.value})}},negated:this.value==null?!negated:negated,label:this.field+': '+report.format(this.value,{}),disabled:false,pinned:false};root.location.assign(report.filterUrl(pill,{filters:this.filters,range:this.range}));}catch(error){this.error=String(error.message || error);}}},
        render(){const h=root.Vue.h,text=this.label ?? report.format(this.value,{});return h('span',{class:'m-report-value-filter','data-report-filter-field':this.field},[this.hideValue?null:this.link?h('a',{href:report.safeLink(this.link)},text):h('span',text),E.button('Filter',()=>{this.open=!this.open;}),this.open?h('span',{role:'menu'},[E.button('Filter for',()=>this.apply(false)),E.button('Filter out',()=>this.apply(true)),E.button('Cancel',()=>{this.open=false;})]):null,this.error?h('span',{role:'alert'},this.error):null]);}
    };
    components["m-report-definition-editor"] = {
        inject: { mForm: { default: null } },
        props: { name: { type: String, default: "definitionJson" }, libraries: { type: Array, default: () => [] }, datasets: { type: Array, default: () => [] }, readOnly: Boolean, panelOnly: Boolean, queryOnly:Boolean },
        data: () => ({ selected: "", optionSearch: "", libraryId: "", error: "",queryAt:0,transformScope:"panel" }),
        computed: {
            body() { try { const value = JSON.parse(this.mForm && this.mForm.fields[this.name] || "{}"); return !value || Array.isArray(value) || typeof value !== "object" ? null : this.queryOnly ? {panels:{panel:{queries:value.queries || [value],transformations:value.queries?value.transformations || []:[]}},queryEnvelope:value.queries?value:{}} : this.panelOnly ? { panels: { panel: value } } : value; } catch (_) { return null; } },
            panels() { return this.body && this.body.panels || {}; },
            selectedId() { return this.selected && this.panels[this.selected] ? this.selected : Object.keys(this.panels)[0]; }
        },
        methods: {
            write(body) { if (!this.readOnly && this.mForm) this.mForm.fields[this.name] = JSON.stringify(this.queryOnly ? Object.assign({},body.queryEnvelope,{queries:body.panels.panel.queries,transformations:body.panels.panel.transformations}) : this.panelOnly ? body.panels.panel : body, null, 2); },
            update(path, value) {
                const body = this.body, id = this.selectedId;
                if (!body || !id || this.panels[id].panelRef) return;
                let target = body.panels[id];
                const parts = path.split(".");
                for (let at=0;at<parts.length-1;at++) { const key=parts[at];target=target[key] || (target[key]=/^\d+$/.test(parts[at+1])?[]:{}); }
                if (value === undefined) delete target[parts[parts.length - 1]];
                else target[parts[parts.length - 1]] = value;
                this.write(body);
            },
            add(copy) {
                const body = this.body;
                if (!body) return;
                body.panels = body.panels || {};
                let id = copy ? this.selectedId + "-copy" : "panel-" + Date.now().toString(36);
                while (body.panels[id]) id += "-copy";
                body.panels[id] = copy ? JSON.parse(JSON.stringify(body.panels[this.selectedId])) : { title: "New panel", visualization: "table", queries: [{ datasetId: "otel_metrics", mode: "documents", pageSize: 100 }] };
                this.selected = id; this.write(body);
            },
            remove() { const body = this.body; if (body && this.selectedId) { delete body.panels[this.selectedId]; this.write(body); } },
            library(unlink) {
                const body = this.body;
                if (!body) return;
                const reference = unlink ? this.panels[this.selectedId].panelRef : this.libraryId;
                const definition = this.libraries.find((item) => item.id === reference);
                if (!definition) return;
                body.panels = body.panels || {};
                const id = unlink ? this.selectedId : "library-" + Date.now().toString(36);
                body.panels[id] = unlink ? JSON.parse(JSON.stringify(definition.body)) : { panelRef: definition.id };
                this.selected = id; this.write(body);
            },
            layout(type) {
                const body = this.body;
                if (!body) return;
                const ids = Object.keys(body.panels || {});
                const auto = { type: "autoGrid", items: ids.map((panelId) => ({ panelId })) };
                body.layout = type === "grid" ? { type, items: ids.map((panelId, at) => ({ panelId, x: at % 2 * 12, y: Math.floor(at / 2), w: 12, h: 1 })) } :
                    type === "rows" ? { type, items: [{ title: "Row", layout: auto }] } : type === "tabs" ? { type, items: [{ title: "Tab", layout: auto }] } : auto;
                this.write(body);
            },
            updateBody(path,value){const body=this.body;if(!body)return;E.set(body,path,value);this.write(body);},
            queries(action){const body=this.body,panel=body && body.panels[this.selectedId];if(!panel || panel.panelRef)return;const queries=panel.queries=panel.queries || [];
                if(action==='add'||action==='duplicate'){queries.push(action==='duplicate' && queries[this.queryAt]?E.clone(queries[this.queryAt]):{datasetId:this.datasets[0] && this.datasets[0].id || 'otel_logs',mode:'documents',pageSize:100});this.queryAt=queries.length-1;}
                if(action==='remove'){queries.splice(this.queryAt,1);this.queryAt=Math.max(0,this.queryAt-1);}
                if(action==='up' && this.queryAt>0){E.move(queries,this.queryAt,-1);this.queryAt--;}
                if(action==='down' && this.queryAt<queries.length-1){E.move(queries,this.queryAt,1);this.queryAt++;}
                this.write(body);
            },
            aggregation(add, at) {
                const body=this.body,panel=body && body.panels[this.selectedId];if (!panel || panel.panelRef) return;
                panel.queries=panel.queries || [{}];const query=panel.queries[Math.min(this.queryAt,panel.queries.length-1)],items=query.aggregations=query.aggregations || [];
                if (add) { query.mode='aggregations';items.push({id:'metric'+(items.length+1),type:'sum',role:'metric',params:{field:'sum'}}); }
                else items.splice(at,1);this.write(body);
            },
            transformation(add, at) {
                const body=this.body,panel=body && body.panels[this.selectedId];if (!panel || panel.panelRef) return;
                const items=panel.transformations=panel.transformations || [];
                if (add) items.push({type:'limit',count:100});else items.splice(at,1);this.write(body);
            },
            position(key, value) {
                const body = this.body;
                if (!body || !body.layout || body.layout.type !== "grid") return;
                const item = (body.layout.items || []).find((item) => item.panelId === this.selectedId);
                if (item) { item[key] = Number(value); this.write(body); }
            }
        },
        render() {
            const h = root.Vue.h;
            if (!this.body) return h("p", { role: "alert" }, "Correct the JSON below to use the panel options.");
            const panel = this.panels[this.selectedId] || {}, options = [];
            const read = (path) => path.split(".").reduce((value, key) => value && value[key], panel);
            const input = (label, path, type, choices) => {
                if (this.optionSearch && !label.toLowerCase().includes(this.optionSearch.toLowerCase())) return;
                options.push(h("label", { class: "q-pa-xs", style: { display: "block" } }, [h("span", { style: { display: "block" } }, label), choices ?
                    h("select", { value: read(path) || "", disabled: this.readOnly, onChange: (event) => this.update(path, event.target.value) }, choices.map((choice) => h("option", { value: choice.value ?? choice }, choice.text || choice.value || choice))) :
                    h("input", { type: type || "text", checked: type === "checkbox" ? !!read(path) : undefined, value: read(path) ?? "", disabled: this.readOnly, onChange: (event) => this.update(path, type === "checkbox" ? event.target.checked : type === "number" ? (event.target.value === "" ? undefined : Number(event.target.value)) : event.target.value) })]));
            };
            const queryAt=Math.min(this.queryAt,Math.max(0,(panel.queries || []).length-1)),queryPrefix='queries.'+queryAt+'.';
            options.push(h('div',{class:'row q-gutter-sm'},[E.control('Selected query',queryAt,value=>{this.queryAt=Number(value);},'select',(panel.queries || []).map((query,at)=>({value:at,text:'Query '+(at+1)+' · '+(query.datasetId || 'choose dataset')})),this.readOnly),E.button('Add query',()=>this.queries('add'),this.readOnly),E.button('Duplicate query',()=>this.queries('duplicate'),this.readOnly || !(panel.queries || []).length),E.button('Remove query',()=>this.queries('remove'),this.readOnly || !(panel.queries || []).length),E.button('Move query up',()=>this.queries('up'),this.readOnly || !queryAt),E.button('Move query down',()=>this.queries('down'),this.readOnly || queryAt>=(panel.queries || []).length-1)]));
            if(!this.queryOnly)input("Panel title", "title");
            if(!this.queryOnly)input("Visualization", "visualization", "select", ["timeSeries", "area", "bar", "stat", "gauge", "barGauge", "table", "pie", "donut", "heatmap", "stateTimeline", "logs", "traceWaterfall", "text"]);
            if(!this.queryOnly && panel.visualization==='text')options.push(E.control('Markdown text',panel.text || '',value=>this.update('text',value),'textarea',null,this.readOnly));
            input("Dataset", queryPrefix+"datasetId", "select", this.datasets.map((dataset) => ({ value: dataset.id, text: dataset.title || dataset.id })));
            input("Query string", queryPrefix+"queryString");
            input("Query mode", queryPrefix+"mode", "select", ["aggregations", "documents"]);
            const aggregationRows=((panel.queries || [])[queryAt] || {}).aggregations || [];
            aggregationRows.forEach((aggregation,at)=>{
                const prefix=queryPrefix+'aggregations.'+at+'.';
                input('Aggregation '+(at+1)+' ID',prefix+'id');
                input('Aggregation '+(at+1)+' type',prefix+'type','select',['value_count','sum','min','max','avg','terms','date_histogram','cardinality','percentiles','histogram_merge']);
                input('Aggregation '+(at+1)+' role',prefix+'role','select',['metric','x-axis','series']);
                input('Aggregation '+(at+1)+' field',prefix+'params.field');
                if (aggregation.type==='terms') {input('Aggregation '+(at+1)+' top values',prefix+'params.size','number');input('Aggregation '+(at+1)+' Missing label',prefix+'params.missing');input('Aggregation '+(at+1)+' Other bucket',prefix+'params.other_bucket','checkbox');input('Aggregation '+(at+1)+' Other label',prefix+'params.other_bucket_key');input('Aggregation '+(at+1)+' Minimum count',prefix+'params.min_doc_count','number');}
                if (aggregation.type==='percentiles') options.push(E.control('Percentiles',aggregation.params && aggregation.params.percents || [50,95,99],value=>this.update(prefix+'params.percents',value.map(Number)),'list',null,this.readOnly));
                if (aggregation.type==='histogram_merge') options.push(E.control('Stored histogram bounds',aggregation.params && aggregation.params.bounds || [],value=>this.update(prefix+'params.bounds',value.map(Number)),'list',null,this.readOnly));
                if (aggregation.type==='date_histogram') input('Aggregation '+(at+1)+' interval',prefix+'params.interval');
                options.push(h('button',{type:'button',disabled:this.readOnly,onClick:()=>this.aggregation(false,at)},'Remove aggregation '+(at+1)));
            });
            options.push(h('button',{type:'button',disabled:this.readOnly,onClick:()=>this.aggregation(true)},'Add aggregation'));
            input('Query time from',queryPrefix+'from');input('Query time through',queryPrefix+'thru');input('Document page size',queryPrefix+'pageSize','number');
            options.push(E.control('Transformations apply to',this.transformScope,value=>{this.transformScope=value;},'select',[{value:'panel',text:'Combined query frames'},{value:'query',text:'Selected query'}],this.readOnly));
            const transformPrefix=this.transformScope==='query'?queryPrefix+'transformations':'transformations';
            options.push(h(components['m-report-transform-editor'],{value:read(transformPrefix) || [],disabled:this.readOnly,onChange:value=>this.update(transformPrefix,value)}));
            if(!this.queryOnly){
            input("Field unit", "fieldConfig.defaults.unit");
            input("Decimal places", "fieldConfig.defaults.decimals", "number");
            input("Field minimum", "fieldConfig.defaults.min", "number");
            input("Field maximum", "fieldConfig.defaults.max", "number");
            input("No-value text", "fieldConfig.defaults.noValue");
            input("Compare previous period", "comparePrevious", "checkbox");
            input("Panel time from override", "time.from"); input("Panel time through override", "time.thru");
            }else input("Compare previous period",queryPrefix+"comparePrevious","checkbox");
            const layout = this.body.layout || {};
            if (layout.type === "grid") {
                const item = (layout.items || []).find((item) => item.panelId === this.selectedId) || {};
                for (const [key, label] of [["x", "Grid column (0–23)"], ["y", "Grid row"], ["w", "Width in columns (1–24)"], ["h", "Height in rows"]]) {
                    if (!this.optionSearch || label.toLowerCase().includes(this.optionSearch.toLowerCase())) options.push(h("label", { class: "q-pa-xs" }, [label,
                        h("input", { type: "number", value: item[key] ?? "", disabled: this.readOnly, onChange: (event) => this.position(key, event.target.value) })]));
                }
            }
            return h("section", { class: "m-report-editor q-pa-sm", style: { border: "1px solid var(--q-secondary)" } }, [h("h4", "Panel options"),
                (this.panelOnly || this.queryOnly) ? null : h("div", { class: "row q-gutter-sm" }, [h("select", { value: this.selectedId || "", onChange: (event) => { this.selected = event.target.value; } }, Object.keys(this.panels).map((id) => h("option", { value: id }, this.panels[id].title || id))),
                    h("button", { type: "button", disabled: this.readOnly, onClick: () => this.add(false) }, "Add panel"),
                    h("button", { type: "button", disabled: this.readOnly || !this.selectedId, onClick: () => this.add(true) }, "Duplicate"),
                    h("button", { type: "button", disabled: this.readOnly || !this.selectedId, onClick: this.remove }, "Remove"),
                    h("label", ["Layout ", h("select", { value: layout.type || "autoGrid", disabled: this.readOnly, onChange: (event) => this.layout(event.target.value) }, ["autoGrid", "grid", "rows", "tabs"].map((value) => h("option", { value }, value)))]) ]),
                (this.panelOnly || this.queryOnly) ? null : h("div", { class: "q-py-sm" }, [h("label", ["Library panel ", h("select", { value: this.libraryId, onChange: (event) => { this.libraryId = event.target.value; } }, [h("option", { value: "" }, "Choose"), ...this.libraries.map((item) => h("option", { value: item.id }, item.title))])]),
                    h("button", { type: "button", disabled: this.readOnly || !this.libraryId, onClick: () => this.library(false) }, "Add library reference")]),
                panel.panelRef ? h("p", ["Library panel: " + panel.panelRef + " ", h("a", { href: report.appPath("/apps/dashboards/Definition?definitionId=" + encodeURIComponent(panel.panelRef)) }, "Edit library panel"), " ",
                    h("button", { type: "button", disabled: this.readOnly, onClick: () => this.library(true) }, "Unlink as copy")]) :
                    h("div", [h("input", { type: "search", placeholder: "Search panel options", value: this.optionSearch, onInput: (event) => { this.optionSearch = event.target.value; } }), ...options]),
                !this.queryOnly && !panel.panelRef && !this.optionSearch ? h('details',[h('summary','Field overrides, mappings and thresholds'),h(components['m-report-field-editor'],{value:panel.fieldConfig || {},disabled:this.readOnly,onChange:value=>this.update('fieldConfig',value)})]):null,
                !this.panelOnly && !this.queryOnly ? h('details',[h('summary','Dashboard variables'),h(components['m-report-variable-editor'],{value:this.body.variables || [],datasets:this.datasets,disabled:this.readOnly,onChange:value=>this.updateBody('variables',value)})]):null,
                !this.panelOnly && !this.queryOnly ? h('details',[h('summary','Nested rows, tabs and panel placements'),h(components['m-report-layout-editor'],{value:this.body.layout || E.layout('autoGrid',Object.keys(this.panels)),panelIds:Object.keys(this.panels),disabled:this.readOnly,onChange:value=>this.updateBody('layout',value)})]):null,
                h("p", "Options update the definition below. Save a version to keep the changes.")]);
        }
    };
    components["m-log-tail"] = {
        props: { url: { type: String, required: true }, params: { type: Object, default: () => ({}) } },
        data: () => ({ active: false, hits: [], cursor: null, error: "", removed: 0, timer: null, loading: false }),
        beforeUnmount() { root.clearInterval(this.timer); },
        methods: {
            toggle() {
                this.active = !this.active; root.clearInterval(this.timer);
                if (this.active) { this.poll(); this.timer = root.setInterval(() => { if (!root.document.hidden) this.poll(); }, 2000); }
            },
            async poll() {
                if (this.loading) return;
                this.loading = true;
                try {
                    const result = await moqui.request(this.url, { params: Object.assign({}, this.params, this.cursor ? { cursor: JSON.stringify(this.cursor) } : {}) });
                    if (!result.json || result.status >= 400) throw new Error(result.json && result.json.error || "Telemetry request failed");
                    const merged = report.mergeLogHits(this.hits, result.json.hits || [], 500);
                    this.hits = merged.hits; this.removed += merged.removed;
                    this.cursor = result.json.nextCursor || this.cursor; this.error = "";
                } catch (error) { this.error = String(error); }
                finally { this.loading = false; }
            }
        },
        render() {
            const h = root.Vue.h;
            return h("section", { class: "q-pa-sm" }, [h("button", { type: "button", class: "q-btn q-pa-xs", "aria-pressed": this.active, onClick: this.toggle }, this.active ? "Pause live tail" : "Start live tail"),
                this.error ? h("p", { role: "alert", class: "text-negative" }, this.error + " — telemetry is unavailable") : null,
                this.removed ? h("p", this.removed + " older entries removed from this view; search or export to read them") : null,
                this.active || this.hits.length ? h("div", { role: "log", "aria-live": "off", style: { maxHeight: "420px", overflow: "auto", fontFamily: "monospace" } },
                    this.hits.length ? this.hits.map((hit) => {
                        const doc = hit.body || {}, trace = doc.trace_id;
                        return h("details", { key: hit.documentId + ":" + hit.id, class: "q-py-xs" }, [
                            h("summary", { style: { color: Number(doc.severity_number) >= 17 ? "var(--q-negative)" : undefined } },
                                report.format(doc.timestamp || doc.start, { type: "time" }) + " " + (doc.severity_text || "") + " " + String(doc.body || doc.name || "").slice(0, 500)),
                            trace ? h("a", { href: report.appPath("/apps/monitor/TraceDetail?traceId=" + encodeURIComponent(trace)) }, "Open trace") : null,
                            h("pre", { style: { whiteSpace: "pre-wrap", overflowWrap: "anywhere" } }, JSON.stringify(doc, null, 2)) ]);
                    }) : [h("p", "No new telemetry. This does not establish health.")]) : null ]);
        }
    };
    components["m-trace-waterfall"] = {
        props: { spans: { type: Array, default: () => [] } }, emits: ["select"],
        data: () => ({ selected: null }),
        render() {
            const h = root.Vue.h, rows = report.waterfall(this.spans);
            if (!rows.length) return h("p", { role: "status" }, "No spans in this time range. Telemetry may be missing.");
            return h("div", { class: "m-trace-waterfall" }, [
                ...rows.map((row) => h("button", { type: "button", class: "full-width text-left q-pa-sm", key: row.span.span_id,
                    style: { display: "grid", gridTemplateColumns: "minmax(180px, 35%) 1fr", border: "0", background: "transparent", color: "inherit" },
                    onClick: () => { this.selected = row.span; this.$emit("select", row.span); },
                    "aria-label": row.span.name + (row.span.status_code === "ERROR" ? ", ERROR" : "") + ", " + row.duration.toFixed(2) + " milliseconds" }, [
                    h("span", { style: { paddingLeft: row.depth * 12 + "px", overflow: "hidden", textOverflow: "ellipsis" } },
                        row.span.name + (row.missingParent ? " (parent unavailable)" : "")),
                    h("span", { style: { position: "relative", height: "22px", borderBottom: "1px solid var(--q-secondary)" } }, [
                        h("span", { style: { position: "absolute", left: row.left + "%", width: row.width + "%", minWidth: "2px", height: "18px",
                            background: row.span.status_code === "ERROR" ? "var(--q-negative)" : "var(--q-primary)", borderRadius: "2px" } }),
                        h("span", { style: { position: "relative", textShadow: "0 0 3px var(--m-panel-bg)", paddingLeft: "4px" } }, row.duration.toFixed(2) + " ms") ]) ])),
                this.selected ? h("details", { open: true, class: "q-pa-sm" }, [h("summary", this.selected.name), h("pre", { style: { whiteSpace: "pre-wrap", overflowWrap: "anywhere" } }, JSON.stringify(this.selected, null, 2))]) : null ]);
        }
    };
    components["m-report-playlist"] = {
        props: { items: {type:Array,default:()=>[]}, seconds: {type:Number,default:30} },
        data: () => ({ at:0, paused:false, timer:null }),
        mounted() { this.timer=root.setInterval(()=>{ if (!this.paused && !root.document.hidden && this.items.length) this.at=(this.at+1)%this.items.length; }, Math.max(5,this.seconds)*1000); },
        beforeUnmount() { if (this.timer) root.clearInterval(this.timer); },
        render() {
            const h=root.Vue.h,item=this.items[this.at];
            if (!item) return h("p",{role:"status"},"No accessible dashboards in this playlist.");
            return h("section",[h("div",{class:"row items-center q-gutter-sm"},[
                h("button",{type:"button",onClick:()=>{this.at=(this.at+this.items.length-1)%this.items.length;}},"Previous"),
                h("button",{type:"button",onClick:()=>{this.paused=!this.paused;}},this.paused?"Resume":"Pause"),
                h("button",{type:"button",onClick:()=>{this.at=(this.at+1)%this.items.length;}},"Next"),
                h("a",{href:report.safeLink(report.appPath(item.url))},item.title),h("span",(this.at+1)+" / "+this.items.length)]),
                h("iframe",{key:this.at,src:report.safeLink(report.appPath(item.url)),title:item.title,style:{width:"100%",height:"80vh",border:0},referrerpolicy:"same-origin"})]);
        }
    };
    components["m-report-panel"] = {
        props: { frame: { type: Object, default: () => ({ fields: [] }) }, frames: Array, panel: { type: Object, default: () => ({}) },
            title: String, height: { type: String, default: "300px" }, dashboardId: String, panelId: String, screenUrl: String, range: Object, variables: Object, syncGroup:String,filters:{type:Array,default:()=>[]} },
        emits:["filter"],
        data: () => ({ sortField: -1, descending: false, inspector: false, inspection: null, busy: false, actionError: "" }),
        methods: {
            async panelRequest(action, extra) {
                this.busy = true; this.actionError = "";
                try {
                    const parameters = new root.URLSearchParams(Object.assign({ definitionId: this.dashboardId, panelId: this.panelId,
                        from: this.range && this.range.from || "now-7d", thru: this.range && this.range.thru || "now", variables: JSON.stringify(this.variables || {}),filtersJson:JSON.stringify(this.filters || []) }, extra || {}));
                    // The transitions of the screen that renders the dashboard (the Monitor screens include them), at its server path.
                    const response = await root.fetch((this.screenUrl || "/apps/dashboards/Dashboard") + "/" + action + "?" + parameters.toString(), { credentials: "same-origin", headers: { Accept: "application/json" } });
                    if (!response.ok) throw new Error("Request failed (" + response.status + ")");
                    const result = await response.json();
                    if (result.errors || result.error) throw new Error(result.errors || result.error);
                    return result;
                } catch (error) { this.actionError = String(error.message || error); return null; }
                finally { this.busy = false; }
            },
            filter(pill){if(!pill)return;if(this.dashboardId)this.$emit('filter',pill);else root.location.assign(report.filterUrl(pill,{range:this.range}));},
            async inspect() {
                this.inspector = !this.inspector;
                if (this.inspector && this.dashboardId && !this.inspection) this.inspection = await this.panelRequest("inspectPanel");
            },
            async exportPanel(renderMode) {
                const result = await this.panelRequest("exportPanel", { renderMode });
                if (!result) return;
                if (!["text/csv", "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"].some(type => String(result.contentType).startsWith(type))) { this.actionError = "Unsupported export response"; return; }
                const bytes = root.atob(result.contentBase64 || ""), data = new Uint8Array(bytes.length);
                for (let at = 0; at < bytes.length; at++) data[at] = bytes.charCodeAt(at);
                const url = root.URL.createObjectURL(new root.Blob([data], { type: result.contentType })), link = root.document.createElement("a");
                link.href = url; link.download = result.filename || "panel." + renderMode; link.click(); root.URL.revokeObjectURL(url);
            },
            csv() { const blob = new root.Blob([report.csv(this.frames && this.frames[0] || this.frame)], { type: "text/csv;charset=utf-8" });
                const url = root.URL.createObjectURL(blob), link = root.document.createElement("a"); link.href = url; link.download = (this.title || this.panel.title || "report") + ".csv";
                link.click(); root.URL.revokeObjectURL(url); }
        },
        render() {
            const h = root.Vue.h, frame = this.frames && this.frames[0] || this.frame;
            const fields = report.configuredFields(frame, this.panel), rows = report.rows(frame), visual = this.panel.visualization || this.panel.type || "table";
            const title = this.title || this.panel.title;
            const named = title || "Report";
            const header = h("div", { class: "row items-center q-gutter-sm" }, [h("strong", { class: "col" }, named),
                h("button", { type: "button", onClick: this.dashboardId ? () => this.exportPanel("csv") : this.csv, disabled: this.busy, class: "q-btn q-pa-xs", "aria-label": "Export " + named + " as CSV" }, "CSV"),
                this.dashboardId ? h("button", { type: "button", onClick: () => this.exportPanel("xlsx"), disabled: this.busy, class: "q-btn q-pa-xs", "aria-label": "Export " + named + " as XLSX" }, "XLSX") : null,
                h("button", { type: "button", onClick: this.inspect, disabled: this.busy, class: "q-btn q-pa-xs", "aria-label": "Inspect " + named, "aria-pressed": this.inspector }, "Inspect"),
                this.dashboardId && (this.panel.queries || []).some((query) => (query.aggregations || []).some((agg) => agg.role === "metric")) ?
                    h("a", { href: report.appPath("/apps/dashboards/AlertRule?dashboardId=" + encodeURIComponent(this.dashboardId) + "&panelId=" + encodeURIComponent(this.panelId)) }, "New alert rule") : null]);
            let content;
            if (visual === "text") content = h(root.Vue.resolveComponent("m-markdown"), { text: this.panel.text || "" });
            else if (!rows.length) content = h("p", { role: "status", class: "text-grey" }, "No data in this time range.");
            else if (visual === "stat" || visual === "gauge" || visual === "barGauge") {
                const series=report.chart(frame,this.panel).data.datasets,previousAt=this.panel.compareField?fields.findIndex(field=>field.name===this.panel.compareField):-1;
                content = h("div", { class: "q-pa-md" }, [!series.length?h('p',{role:'status'},'No numeric metrics in this frame.'):null,...series.slice(0,50).map((dataset,index)=>{
                    let latest=dataset._reportRows.length-1;while(latest>=0 && dataset._reportRows[latest]==null)latest--;
                    const sourceRow=latest>=0?dataset._reportRows[latest]:null,value=latest>=0?dataset.data[latest]:null,field=Object.assign({},dataset._reportField,dataset._reportField.config || {}),previous=sourceRow!=null && previousAt>=0?rows[sourceRow][previousAt]:null;
                    return h('div',{class:'m-report-stat-series q-mb-md',key:index,'data-report-series':dataset.label},[h('div',{class:'text-subtitle2'},dataset.label),h('div',{class:'text-h4',style:{color:report.thresholdColor(value,field)}},report.format(value,field)),
                        finite(previous) && Number(previous)!==0 && finite(value)?h('p',((Number(value)-Number(previous))/Math.abs(Number(previous))*100).toFixed(1)+'% from previous period'):null,
                        visual!=='stat'?(finite(value)?h('meter',{min:field.min??0,max:field.max??100,value,style:{width:'100%'},'aria-label':dataset.label}):null):
                            h('svg',{viewBox:'0 0 100 24',height:'36',width:'100%',role:'img','aria-label':'Recent values for '+dataset.label},[h('path',{d:report.sparkline(dataset.data),fill:'none',stroke:'var(--q-primary)','stroke-width':'1'})])]);
                }),series.length>50?h('p',{role:'status'},'Showing 50 of '+series.length+' series. Narrow the query to inspect the remaining series.'):null]);
            } else if (["timeSeries", "timeseries", "line", "area", "bar", "pie", "donut"].includes(visual)) {
                content = h(root.Vue.resolveComponent("m-chart"), { config: report.chart(frame, Object.assign({},this.panel,{syncGroup:this.syncGroup}),this.filter), height: this.height });
            } else if (visual === "traceWaterfall" || visual === "trace") {
                content = h(components["m-trace-waterfall"], { spans: rows.map((row) => Object.fromEntries(fields.map((field, at) => [field.name, row[at]]))) });
            } else if (visual === "heatmap") {
                const values = rows.map((row) => row.filter(finite).map(Number));
                const max = Math.max(1, ...values.flat());
                content = h("div", { style: { display: "grid", gridTemplateColumns: "repeat(" + Math.max(1, fields.length) + ",1fr)", gap: "2px" } },
                    rows.flatMap((row, at) => row.map((value, index) => h("div", { key: at + ":" + index, title: fields[index].name + ": " + report.format(value, fields[index]),
                        style: { padding: "4px", background: finite(value) ? "var(--q-primary)" : "transparent", opacity: finite(value) ? Math.max(0.15, Number(value) / max) : 1 } }, report.format(value, fields[index])))));
            } else if (visual === "stateTimeline") {
                content = h("div", rows.map((row, at) => h("div", { key: at, class: "row no-wrap" }, row.map((value, index) => h("span", {
                    style: { padding: "8px", flex: index ? 1 : undefined, background: index ? report.thresholdColor(value, fields[index]) || "var(--q-secondary)" : "transparent" },
                    title: fields[index].name + ": " + String(value ?? "Missing") }, report.format(value, fields[index]))))));
            } else {
                const visible = fields.map((field, index) => ({ field, index })).filter(({ field }) => !field.hidden && !(field.config && field.config.hidden));
                const sorted = rows.map((row,sourceRow) => Object.assign(row.slice(),{sourceRow}));
                if (this.sortField >= 0) sorted.sort((a, b) => {
                    const av = a[this.sortField], bv = b[this.sortField];
                    const n = av == null ? (bv == null ? 0 : 1) : bv == null ? -1 : finite(av) && finite(bv) ? Number(av) - Number(bv) : String(av).localeCompare(String(bv));
                    return this.descending ? -n : n;
                });
                content = h("div", { class: "scroll" }, [h("table", { class: "q-table full-width" }, [
                    h("thead", [h("tr", visible.map(({ field, index }) => h("th", { style: { textAlign: "left" }, "aria-sort": this.sortField === index ? (this.descending ? "descending" : "ascending") : "none" }, [
                        h("button", { type: "button", onClick: () => { this.descending = this.sortField === index ? !this.descending : false; this.sortField = index; } }, field.displayName || field.name) ])))]),
                    h("tbody", sorted.map((row, at) => h("tr", { key: at }, visible.map(({ field, index }) => {
                        const value = row[index], text = report.cellText(value, field,row.sourceRow), record = Object.fromEntries(fields.map((item, i) => [item.name, row[i]]));
                        const link = field.link ? report.dataLink(field.link, value, record) : "";
                        return h("td", { style: { color: report.thresholdColor(value, field), ...(field.frozen ? { position: "sticky", left: "0", background: "var(--m-panel-bg)" } : {}) } },
                            link ? h("a", { href: report.appPath(link) }, text) : report.bucketPill(frame,this.panel,index,row.sourceRow,false) ? h('button',{type:'button',class:'m-report-cell-filter',title:'Filter for this value; Shift-click filters out',onClick:event=>this.filter(report.bucketPill(frame,this.panel,index,row.sourceRow,event.shiftKey))},text) : text.length > 240 ? h("details", [h("summary", text.slice(0, 200) + "…"), h("pre", { style: { whiteSpace: "pre-wrap" } }, text)]) : text);
                    })))),
                    this.panel.totals ? h("tfoot", [h("tr", visible.map(({ field, index }) => h("td", report.format(this.panel.totals[field.name] ?? null, field))))]) : null ]) ]);
            }
            const annotations = (this.panel.annotations || []).slice(0, 200);
            return h("section", { class: "m-report-panel q-pa-sm q-mb-sm", style: { border: "1px solid var(--q-secondary)", borderRadius: "4px" } },
                [header, this.actionError ? h("p", { role: "alert", class: "text-negative" }, this.actionError) : null, content, annotations.length ? h("details", [h("summary", annotations.length + " annotations"), h("ul", annotations.map((annotation) => h("li", report.format(annotation.time, { type: "time" }) + " · " + annotation.text)))]) : null,
                this.inspector ? h("pre", { style: { whiteSpace: "pre-wrap", overflowWrap: "anywhere" } }, JSON.stringify(this.inspection || { panel: this.panel, frame }, null, 2)) : null]);
        }
    };
    components['m-dashboard-filters']={
        props:{dashboard:{type:Object,required:true}},emits:['apply'],
        data(){return {dataset:Object.keys(this.dashboard.datasetFields || {})[0] || '',form:{fields:{queryString:'',filtersJson:JSON.stringify(this.dashboard.filters || [])}}};},
        provide(){return {mForm:this.form};},
        render(){const h=root.Vue.h,fields=this.dashboard.datasetFields || {};return h('details',[h('summary','Dashboard filters ('+report.parsePills(this.form.fields.filtersJson).length+')'),E.control('Filter dataset',this.dataset,value=>{this.dataset=value;},'select',Object.keys(fields)),h(components['m-report-querybar'],{filterOnly:true,fields:fields[this.dataset] || [],datasetId:this.dataset,viewerId:this.dashboard.viewerId}),E.button('Apply dashboard filters',()=>this.$emit('apply',report.parsePills(this.form.fields.filtersJson))),h('p','Filters are enforced by the server for matching datasets and included in inspection and exports.')]);}
    };
    components["m-dashboard"] = {
        props: { dashboard: { type: Object, default: () => ({ panels: {} }) }, frames: { type: Object, default: () => ({}) } },
        data() { return { syncGroup:"report-dashboard-"+Math.random().toString(36).slice(2), keyPrefix:"", keyAt:0, panelRefs:{}, selectedPanel:"", activeTabs: {}, kiosk: !!(this.dashboard && this.dashboard.kiosk), refreshSeconds: 0, timer: null }; },
        mounted() {
            this.refreshSeconds = Number(this.dashboard.refreshSeconds) || 0;
            this.setRefresh();
            this.onKey = (event) => {
                if (event.key === "Escape" && this.kiosk) { event.preventDefault(); this.kiosk = false; return; }
                if (/INPUT|TEXTAREA|SELECT/.test(event.target.tagName) || event.target.isContentEditable) return;
                // Esc in an open dialog closes the dialog only.
                if (event.key === "Escape" && this.dashboard.panelId && !event.defaultPrevented && !root.document.querySelector(".q-dialog")) { event.preventDefault(); this.allPanels(); return; }
                if (event.key === "k" && event.altKey) { event.preventDefault(); this.kiosk = !this.kiosk;return; }
                const prefix=Date.now()-this.keyAt<2000?this.keyPrefix:"";this.keyPrefix="";
                if (prefix==='t') {const range=report.shiftRange(this.dashboard.absoluteTime,event.key);if(range){event.preventDefault();this.timeRange(range);return;}}
                if (prefix==='d' && event.key==='k') {event.preventDefault();this.kiosk=!this.kiosk;return;}
                if (event.ctrlKey || event.metaKey || event.altKey)return;
                if (event.key==='t'||event.key==='d') {this.keyPrefix=event.key;this.keyAt=Date.now();event.preventDefault();return;}
                const panels=this.dashboard.panels || {},id=this.selectedPanel || Object.keys(panels)[0];
                if (event.key==='i' && this.panelRefs[id]) {event.preventDefault();this.panelRefs[id].inspect();}
                if (event.key==='v' && id && this.dashboard.definitionId) {event.preventDefault();const url=new root.URL(root.location.href);url.searchParams.set('panelId',id);root.location.assign(url.href);}
                if (event.key==='e' && this.dashboard.definitionId) {event.preventDefault();root.location.assign(report.appPath('/apps/dashboards/Definition?definitionId='+encodeURIComponent(this.dashboard.definitionId)));}
            };
            root.document.addEventListener("keydown", this.onKey);
            this.onRange=(event)=>{if(event.detail.group!==this.syncGroup)return;const url=new root.URL(root.location.href);url.searchParams.set('from',event.detail.from);url.searchParams.set('thru',event.detail.thru);url.searchParams.delete('cursor');url.searchParams.delete('pageIndex');root.location.assign(url.href);};
            root.document.addEventListener('moqui-report-range',this.onRange);
        },
        beforeUnmount() { root.clearInterval(this.timer); root.document.removeEventListener("keydown", this.onKey);root.document.removeEventListener("moqui-report-range",this.onRange); },
        methods: {
            allPanels(){root.location.assign(report.allPanelsUrl());},
            applyFilters(pills){const url=new root.URL(root.location.href);url.searchParams.set('filtersJson',JSON.stringify(pills));url.searchParams.delete('cursor');url.searchParams.delete('pageIndex');root.location.assign(url.href);},
            addFilter(pill){root.location.assign(report.filterUrl(pill,{dashboardId:this.dashboard.definitionId,filters:this.dashboard.filters,range:this.dashboard.absoluteTime}));},
            timeRange(range) {const url=new root.URL(root.location.href);url.searchParams.set('from',range.from);url.searchParams.set('thru',range.thru);url.searchParams.delete('cursor');url.searchParams.delete('pageIndex');root.location.assign(url.href);},
            setRefresh() {
                root.clearInterval(this.timer);
                if (this.refreshSeconds >= 5) this.timer = root.setInterval(() => {
                    if (!root.document.hidden) root.location.reload();
                }, this.refreshSeconds * 1000);
            },
            variable(variable, value) {
                const url = new root.URL(root.location.href), name = "var-" + variable.name;
                url.searchParams.delete(name);
                for (const item of Array.isArray(value) ? value : [value]) url.searchParams.append(name, item);
                root.location.assign(url.href);
            },
            share() {
                const url = new root.URL(root.location.href), time = this.dashboard.absoluteTime || {};
                if (time.from) url.searchParams.set("from", time.from);
                if (time.thru) url.searchParams.set("thru", time.thru);
                if (root.navigator.clipboard) root.navigator.clipboard.writeText(url.href);
            }
        },
        render() {
            const h = root.Vue.h, panels = this.dashboard.panels || {};
            const entries = Array.isArray(panels) ? panels.map((panel, at) => [panel.id || String(at), panel]) : Object.entries(panels);
            const byId = Object.fromEntries(entries);
            const panel = (id, position) => byId[id] ? h("div", { key: id, style: position || {},onFocusin:()=>{this.selectedPanel=id;},onMouseenter:()=>{this.selectedPanel=id;} }, [h(components["m-report-panel"], { ref:instance=>{this.panelRefs[id]=instance;},panel: byId[id], frames: this.frames[id] || byId[id].frames,
                frame: byId[id].frame || { fields: [] }, dashboardId: this.dashboard.definitionId, panelId: id, screenUrl: this.dashboard.screenUrl, range: this.dashboard.absoluteTime, variables: this.dashboard.variableValues, syncGroup:this.syncGroup,filters:this.dashboard.filters || [],onFilter:this.addFilter })]) : h("p", { role: "alert" }, "Panel " + id + " is unavailable");
            const auto = (items) => h("div", { style: { display: "grid", gridTemplateColumns: "repeat(auto-fit,minmax(min(100%,360px),1fr))", gap: "12px" } }, items);
            const layout = (node, path) => {
                if (!node) return auto(entries.map(([id]) => panel(id)));
                if (node.panelId) return panel(node.panelId);
                const children = node.items || node.children || [];
                if (node.type === "grid") return h("div", { style: { display: "grid", gridTemplateColumns: "repeat(24,minmax(0,1fr))", gap: "12px" } }, children.map((item) => panel(item.panelId,
                    { gridColumn: (Math.max(0, Math.min(23, item.x || 0)) + 1) + "/span " + Math.max(1, Math.min(24, item.w || 12)), gridRow: (Math.max(0, item.y || 0) + 1) + "/span " + Math.max(1, item.h || 1), minWidth: "0" })));
                if (node.type === "tabs") {
                    const at = this.activeTabs[path] || 0;
                    return h("section", [h("div", { role: "tablist" }, children.map((item, index) => h("button", { type: "button", role: "tab", "aria-selected": at === index,
                        onClick: () => { this.activeTabs[path] = index; }, class: "q-btn q-pa-sm" }, item.title || "Tab " + (index + 1)))),
                    children[at] ? h("div", { role: "tabpanel" }, [layout(children[at].layout || children[at], path + ":" + at)]) : null]);
                }
                if (node.type === "rows" || node.type === "row") return h("div", children.map((item, index) => h("details", { open: item.collapsed !== true }, [
                    h("summary", { class: "q-pa-sm text-weight-bold" }, item.title || "Row " + (index + 1)), layout(item.layout || { type: "autoGrid", items: item.items || [] }, path + ":" + index) ])));
                return auto(children.length ? children.map((item, index) => typeof item === "string" ? panel(item) : layout(item, path + ":" + index)) : entries.map(([id]) => panel(id)));
            };
            const variables = (this.dashboard.variables || []).filter((variable) => variable.kind !== "constant").map((variable) => {
                const value = this.dashboard.variableValues && this.dashboard.variableValues[variable.name] || variable.default || "";
                const options = variable.options || [];
                return h("label", { class: "q-pa-xs" }, [h("span", variable.label || variable.name), options.length ? h("select", { multiple: !!variable.multi,
                    value, onChange: (event) => this.variable(variable, variable.multi ? Array.from(event.target.selectedOptions).map((option) => option.value) : event.target.value) },
                    [...(variable.includeAll ? [h("option", { value: "$__all" }, "All")] : []), ...options.map((item) => h("option", { value: typeof item === "object" ? item.value : item }, typeof item === "object" ? item.text || item.value : item))]) :
                    h("input", { value: Array.isArray(value) ? value.join(",") : value, onChange: (event) => this.variable(variable, event.target.value) })]);
            });
            const toolbar = this.kiosk ? null : h("div", { class: "row items-center q-gutter-sm q-py-sm" }, [...variables,
                h("label", ["Refresh ", h("select", { value: this.refreshSeconds, onChange: (event) => { this.refreshSeconds = Number(event.target.value); this.setRefresh(); } },
                    [0, 5, 15, 30, 60, 300].map((seconds) => h("option", { value: seconds }, seconds ? seconds + "s" : "Off")))]),
                h("button", { type: "button", class: "q-btn q-pa-xs", onClick: () => root.location.reload() }, "Refresh now"),
                h("span", "Charts share a cursor. Drag a time range to zoom all panels; double click to reset chart zoom."),
                h("button", { type: "button", class: "q-btn q-pa-xs", onClick: this.share }, "Copy absolute-time link"),
                h("button", { type: "button", class: "q-btn q-pa-xs", onClick: () => { this.kiosk = true; } }, "Kiosk (Alt+K)") ]);
            const single = !this.kiosk && this.dashboard.panelId ? h("p", { class: "row items-center q-gutter-sm" }, [h("span", "Showing panel " + ((byId[this.dashboard.panelId] || {}).title || this.dashboard.panelId) + "."),
                h("button", { type: "button", class: "q-btn q-pa-xs", onClick: this.allPanels }, "All panels (Esc)")]) : null;
            return h("div", { class: "m-report-dashboard", style: this.kiosk ? { position: "fixed", inset: "0", zIndex: "10000", overflow: "auto", background: "var(--m-panel-bg,white)", padding: "12px" } : {} },
                [toolbar, single, !this.kiosk && Object.keys(this.dashboard.datasetFields || {}).length ? h(components["m-dashboard-filters"],{dashboard:this.dashboard,onApply:this.applyFilters}):null, this.kiosk ? h("button", { type: "button", onClick: () => { this.kiosk = false; } }, "Exit kiosk (Esc)") : null, layout(this.dashboard.layout, "root")]);
        }
    };
    if (moqui.qvt && moqui.qvt.registerLate) Object.entries(components).filter(([name]) => ["m-report-value-filter","m-dashboard-filters","m-report-transform-editor","m-report-field-editor","m-report-variable-editor","m-report-layout-editor","m-report-composition-editor","m-report-querybar", "m-report-playlist", "m-report-panel", "m-dashboard", "m-trace-waterfall", "m-log-tail", "m-report-definition-editor"].includes(name))
        .forEach(([name, component]) => moqui.qvt.registerLate(name, component));
})(typeof window !== "undefined" ? window : globalThis);

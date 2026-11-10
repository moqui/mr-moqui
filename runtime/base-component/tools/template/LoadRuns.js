// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.
//
// The reads Tools › Load's screens share (plans/LoadRunner.md §8), over org.moqui.dev.LoadServices, which check
// LOAD_RUNNER and the Tools authorization themselves:
//
// - runView(runId): one run as its page shows it: a running load's snapshot or a finished run's result, made into the
//   header, the totals, the series of the charts, the table by name, the first errors, the generator and, for a load
//   running on this server, the server's numbers from the status map. A run that is not here is { found: false }.
//   While it runs, targets has each scenario's users and rate now, which updateUsers (update#LoadRunUsers) changes.
// - planForm(scriptLocation, runId): what New run asks for a script: the targets its plan's uses allow, each param by
//   its kind, each scenario's settings. From a run (Run again) the script and target are the run's, resolved again
//   against the scripts of the loaded components; start#LoadRun checks them again. A secret is never filled, and an
//   imported run (a file from another machine) fills only the target kind: its URL, params and settings are not this
//   server's to reuse.
//   A scenario's settings come from the run's scenarioSettings (as it used them), else the plan's defaults.
// - startParameters(form): start#LoadRun's parameters from a posted form, as JSON (the qvue) or one field a param
//   and setting (the terminal; a secret param's field is password_<name>, so a refused form's parameters, which the
//   session keeps for the form shown again, never hold it).
//
// Every text here comes from a script or a result file and is shown as text, never as HTML.

const S = 'org.moqui.dev.LoadServices.'
// the settings of a scenario a run may override, all whole numbers of seconds or counts
const SETTINGS = ['users', 'rampUp', 'duration', 'iterations']
// what else a scenario's settings may carry from a run (an arrival rate, stages of users over time), passed on as they are
const CARRIED = ['rate', 'stages']
// the first errors a run's page lists
const FIRST_ERRORS = 20

// a service's out-parameters and its errors as text, the errors cleared
function call(verb, parameters) {
    let out = {}
    let error = null
    try {
        out = ec.service.sync().name(S + verb).parameters(parameters || {}).call() || {}
    } catch (e) {
        error = String(e.message || e)
    }
    if (ec.message.hasError()) error = ec.message.errorsString
    ec.message.clearErrors()
    return { out, error }
}

// a number of a result: a JS number, or a decimal of a service's answer
function number(value) {
    if (value == null || value === '') return null
    const parsed = typeof value == 'number' ? value : typeof value.toNumber == 'function' ? value.toNumber() : Number(String(value))
    return Number.isFinite(parsed) ? parsed : null
}
const text = (value) => (value == null ? '' : String(value))
// a time of a run (an RFC 3339 text or epoch milliseconds) in the user's time zone
function timeText(value) {
    if (value == null || value === '') return ''
    try {
        return String(ec.l10n.format(basicConvert(String(value), 'Timestamp'), 'yyyy-MM-dd HH:mm:ss'))
    } catch (e) {
        return String(value)
    }
}

// the outcomes of a name as one text: "ok 12, refused 1"
function outcomesText(outcomes) {
    return Object.entries(outcomes || {}).map(([outcome, count]) => outcome + ' ' + count).join(', ')
}

// the table by name, in the order the names were first seen
function nameRows(names, order) {
    const known = names || {}
    const list = Array.isArray(order) && order.length ? order.filter((name) => known[name]) : Object.keys(known)
    return list.map((name) => {
        const entry = known[name] || {}
        return {
            name: text(name), kind: text(entry.kind), count: number(entry.count), errors: number(entry.errors),
            mean: number(entry.mean), p50: number(entry.p50), p90: number(entry.p90), p95: number(entry.p95),
            p99: number(entry.p99), max: number(entry.max), outcomes: outcomesText(entry.outcomes),
        }
    })
}

// the bins as per-second series: t is the bin's start in seconds from the run's
function series(bins, binSeconds) {
    const seconds = number(binSeconds) || 1
    const list = Array.isArray(bins) ? bins : []
    const round = (value) => (value == null ? null : Math.round(value * 100) / 100)
    return {
        binSeconds: seconds,
        t: list.map((bin) => number(bin.t) || 0),
        rps: list.map((bin) => round((number(bin.requests) || 0) / seconds)),
        eps: list.map((bin) => round((number(bin.errors) || 0) / seconds)),
        p50: list.map((bin) => number(bin.p50)),
        p95: list.map((bin) => number(bin.p95)),
        p99: list.map((bin) => number(bin.p99)),
        users: list.map((bin) => number(bin.users)),
        // a rate scenario's scheduled starts that no user took, per second
        dropped: list.map((bin) => round((number(bin.dropped) || 0) / seconds)),
    }
}

// CPU load and memory of this server, from the status map
function serverNumbers() {
    try {
        const status = ec.factory.getStatusMap(false) || {}
        const system = status.System || {}
        const process = status.Process || {}
        return {
            load: number(system.Load), processors: number(system.Processors), loadPercent: number((status.Utilization || {}).LoadPercent),
            memoryUsed: process.MemoryUsed == null ? '' : text(process.MemoryUsed), threads: number(process.Threads),
        }
    } catch (e) {
        return null
    }
}

// the title of a script, when it is a load script of a loaded component
function scriptTitle(location) {
    const found = (call('find#LoadScripts').out.scripts || []).find((script) => script.location == location)
    return found ? text(found.title || found.name) : ''
}

function runView(runId, options) {
    const id = text(runId)
    const status = call('get#LoadRunStatus', { runId: id })
    if (status.error) return { runId: id, found: false, error: status.error }
    const running = status.out.running === true
    const summary = status.out.summary || {}
    let source = status.out.snapshot || {}
    let run = summary
    if (!running) {
        const read = call('get#LoadRunResult', { runId: id })
        if (read.error || !read.out.result) return { runId: id, found: false, error: read.error || 'Load run ' + id + ' is not on this server' }
        source = read.out.result
        run = Object.assign({}, summary, source.run || {})
    }
    const latency = source.latency || {}
    const onThisServer = run.targetKind == 'service' || run.targetKind == 'this'
    const generator = source.generator || null
    const errorSamples = (Array.isArray(source.errorSamples) ? source.errorSamples : []).slice(0, FIRST_ERRORS).map((sample) => ({
        name: text(sample.name), status: sample.status == null ? '' : text(sample.status), scenario: text(sample.scenario),
        user: sample.user == null ? '' : text(sample.user), message: text(sample.message),
    }))
    return {
        runId: id,
        found: true,
        running,
        status: text(status.out.status || run.status),
        stopping: running && source.stopping === true,
        script: text(run.script),
        scriptTitle: options && options.withTitle ? scriptTitle(run.script) : '',
        label: text(run.label),
        user: text(run.user),
        importedUser: text(run.importedUser),
        imported: run.imported === true,
        target: text(run.target),
        targetKind: text(run.targetKind),
        insecure: run.insecure === true,
        start: text(run.start || source.start),
        end: text(run.end),
        startText: timeText(run.start || source.start),
        endText: timeText(run.end),
        stopReason: text(run.stopReason || run.stoppedBecause),
        totals: {
            users: number(source.users), activeUsers: running ? number(source.activeUsers) : 0,
            requests: number(source.requests), iterations: number(source.iterations), rps: number(source.rps),
            errors: number(source.errors), errorRate: number(source.errorRate), dropped: number(source.dropped),
            p50: number(latency.p50), p95: number(latency.p95), p99: number(latency.p99),
            duration: number(running ? source.elapsed : source.duration),
        },
        series: series(source.bins, source.binSeconds),
        names: nameRows(source.names, source.nameOrder),
        errorSamples,
        generator: generator ? {
            where: text(generator.where), cores: number(generator.cores), cpu: number(generator.cpu), scriptShare: number(generator.scriptShare),
        } : null,
        passed: source.passed == null ? null : source.passed === true,
        thresholds: (Array.isArray(source.thresholds) ? source.thresholds : []).map((limit) => ({
            name: text(limit.name), metric: text(limit.metric), limit: number(limit.limit), value: number(limit.value), passed: limit.passed === true,
        })),
        server: running && onThisServer ? serverNumbers() : null,
        // each scenario's target now, while it runs: users (a rate scenario's most at once) and a rate scenario's rate
        targets: running ? Object.entries(source.targets || {}).map(([name, target]) => ({
            name: text(name), users: number((target || {}).users), rate: (target || {}).rate == null ? null : number(target.rate),
        })) : [],
        // whether a scenario starts its iterations at a rate, so starts can be dropped: a finished run's stats give a
        // rate scenario (and only one) its dropped count, whether it ran from here, from the command or was imported
        hasRate: running ? Object.values(source.targets || {}).some((target) => target && target.rate != null)
            : Object.values(source.scenarios || {}).some((scenario) => scenario && scenario.dropped != null),
    }
}

// a param's kind on the form: a drop-down of its options, a password, a number, a check, or text
function paramKind(param) {
    if (param.secret) return 'password'
    if (Array.isArray(param.options) && param.options.length) return 'options'
    if (typeof param.default == 'number' || param.type == 'number') return 'number'
    if (typeof param.default == 'boolean' || param.type == 'boolean') return 'boolean'
    return 'text'
}

function planForm(scriptLocation, runId) {
    let location = text(scriptLocation)
    let from = null
    let fromSettings = {}
    let imported = false
    if (runId) {
        const read = call('get#LoadRunResult', { runId: text(runId) })
        if (read.error || !read.out.result) return { found: false, error: read.error || 'Load run ' + runId + ' is not on this server' }
        from = read.out.result.run || {}
        imported = from.imported === true
        // the settings the run used; an older result has only its scenarios' counts, of which users is a setting
        const counts = read.out.result.scenarios || {}
        fromSettings = imported ? {} : from.scenarioSettings || Object.fromEntries(Object.entries(counts).map(([name, ran]) => [name, { users: (ran || {}).users }]))
        location = text(from.script)
    }
    const listed = call('find#LoadScripts')
    if (listed.error) return { found: false, error: listed.error }
    // the run's script only when it is still a load script of a loaded component
    const script = (listed.out.scripts || []).find((candidate) => candidate.location == location)
    if (!script) return { found: false, error: location ? location + ' is not a load script of a loaded component' : 'No load script was named' }
    if (script.error) return { found: false, error: text(script.error), scriptLocation: location }
    const uses = Array.isArray(script.uses) && script.uses.length ? script.uses.map(String) : ['http']
    const targets = uses.includes('service') ? ['service'] : ['this', 'url']
    const given = (from && !imported && from.params) || {}
    const params = Object.entries(script.params || {}).map(([name, param]) => {
        const kind = paramKind(param || {})
        const value = kind == 'password' ? '' : given[name] != null ? given[name] : param.default
        return {
            name: text(name), kind, description: text(param.description), required: param.required === true,
            options: kind == 'options' ? param.options.map((option) => text(option)) : [],
            value: value == null ? '' : kind == 'boolean' ? (value === true || String(value) == 'true') : text(value),
        }
    })
    const scenarios = (Array.isArray(script.scenarios) ? script.scenarios : []).map((scenario) => {
        const ran = fromSettings[scenario.name] || {}
        const pick = (key) => (ran[key] !== undefined ? number(ran[key]) : number(scenario[key]))
        const out = { name: text(scenario.name), users: pick('users'), rampUp: pick('rampUp'), duration: pick('duration'), iterations: pick('iterations') }
        const carried = {}
        for (const key of CARRIED) if (ran[key] != null) carried[key] = ran[key]
        if (Object.keys(carried).length) out.carried = carried
        return out
    })
    const targetKind = from && targets.includes(text(from.targetKind)) ? text(from.targetKind) : targets[0]
    return {
        found: true,
        fromRunId: runId ? text(runId) : '',
        imported,
        scriptLocation: location,
        componentName: text(script.componentName),
        name: text(script.name),
        title: text(script.title || script.name),
        description: text(script.description),
        uses,
        targets,
        targetKind,
        targetUrl: targetKind == 'url' && from && !imported ? text(from.target) : '',
        insecure: !!(from && !imported && from.insecure === true),
        label: from ? text(from.label) : '',
        params,
        scenarios,
    }
}

// the scenarios as start#LoadRun takes them: each setting a number, an empty one left to the script's default
function scenarioSettings(scenarios) {
    const out = {}
    for (const [name, settings] of Object.entries(scenarios || {})) {
        const one = {}
        for (const key of SETTINGS) {
            const value = settings == null ? null : settings[key]
            if (value == null || value === '') continue
            const parsed = number(value)
            one[key] = parsed == null ? value : parsed
        }
        for (const key of CARRIED) if (settings && settings[key] != null && settings[key] !== '') one[key] = settings[key]
        out[name] = one
    }
    return out
}

// start#LoadRun's parameters from a form: parametersJson and scenariosJson (the qvue), else p_<param> and
// s_<scenario>_<setting> fields with the scenarios named in scenarioNames (the terminal)
function startParameters(form) {
    const json = (value) => {
        if (value == null || value === '') return {}
        try {
            const parsed = JSON.parse(String(value))
            return parsed && typeof parsed == 'object' && !Array.isArray(parsed) ? parsed : {}
        } catch (e) {
            throw new Error('Not JSON: ' + String(value).slice(0, 40))
        }
    }
    let parameters = json(form.parametersJson)
    let scenarios = json(form.scenariosJson)
    if (!form.parametersJson && !form.scenariosJson) {
        parameters = {}
        scenarios = {}
        for (const key of Object.keys(form.fields || {})) {
            if (key.startsWith('p_')) parameters[key.slice(2)] = form.fields[key]
            else if (key.startsWith('password_')) parameters[key.slice(9)] = form.fields[key]
        }
        let names = []
        try {
            names = JSON.parse(text(form.scenarioNames) || '[]')
        } catch (e) {
            throw new Error('Not JSON: scenarioNames')
        }
        for (const name of Array.isArray(names) ? names.map(String) : []) {
            scenarios[name] = {}
            for (const setting of SETTINGS) scenarios[name][setting] = form.fields['s_' + name + '_' + setting]
            // a setting the form does not show, kept from the run (rate, stages), as JSON
            const carried = form.fields['c_' + name]
            if (carried) Object.assign(scenarios[name], json(carried))
        }
    }
    // an empty param is the script's default; a secret left empty is the script's to refuse
    for (const name of Object.keys(parameters)) if (parameters[name] === '' || parameters[name] == null) delete parameters[name]
    return {
        scriptLocation: text(form.scriptLocation),
        targetKind: text(form.targetKind) || 'this',
        targetUrl: form.targetKind == 'url' ? text(form.targetUrl) : null,
        parameters,
        scenarios: scenarioSettings(scenarios),
        label: text(form.label) || null,
        insecure: String(form.insecure) == 'true',
    }
}

// start a run from a form: { runId } or { error }
function startRun(form) {
    let parameters
    try {
        parameters = startParameters(form)
    } catch (e) {
        return { error: String(e.message || e) }
    }
    const started = call('start#LoadRun', parameters)
    return started.error ? { error: started.error } : { runId: text(started.out.runId) }
}

// change a running scenario's users or rate (update#LoadRunUsers): an error text, or null
function updateUsers(runId, scenario, users, rate) {
    const parameters = { runId: text(runId), scenario: text(scenario) }
    if (users != null && users !== '') parameters.users = users
    if (rate != null && rate !== '') parameters.rate = rate
    if (parameters.users == null && parameters.rate == null) return 'Give the users or the rate'
    return call('update#LoadRunUsers', parameters).error
}

;({ runView, planForm, startRun, updateUsers, call, number })

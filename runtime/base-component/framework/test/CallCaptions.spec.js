// Call captions (org.moqui.impl.CallCaptionServices and CallCaptions.js; plans/Comms-Framework.md §6.4, W6): the policy
// switch, someone not in the call, an end-to-end encrypted call, and turning captions on and off. Every refusal uses the
// same parameters as the case that works. The media node's side (chunking, order, timeouts, backpressure) is
// crates/moqui-host rtc/captions_tests.rs; this spec's media node is a row, so no audio goes anywhere.
// Run: ./moq test framework --spec CallCaptions
const A = "CAPTIONS_SPEC_A"
const B = "CAPTIONS_SPEC_B"
const C = "CAPTIONS_SPEC_C"
const NODE = "captions-spec-node"
const SESSION = "moqui.call.CallSession"
const LEG = "moqui.call.CallLeg"
const MODEL = "captions-spec-whisper"
const ENDPOINT = "captions-spec-audio"

function sys(run) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}
function as(userId, name, parameters) {
    ec.user.internalLoginUser(userId.toLowerCase().replace(/_/g, "."))
    let out = {}
    let thrown = ""
    try {
        // as the Calls screen's transitions call them: the services check the signed-in user's part in the call
        out = sys(() => ec.service.sync().name(name).parameters(parameters || {}).call()) || {}
    } catch (err) {
        thrown = String(err.message || err)
    }
    const errors = thrown + (ec.message.hasError() ? ec.message.errorsString : "")
    ec.message.clearErrors()
    return Object.assign({errors}, out)
}
const captions = (userId, verb, parameters) => as(userId, "org.moqui.impl.CallCaptionServices." + verb, parameters)
const on = (run) => withProperty("call_captions_enabled", "true", run)
function makeUser(userId) {
    sys(() => {
        if (ec.entity.find("moqui.security.UserAccount").condition("userId", userId).count() > 0) return
        const account = ec.entity.makeValue("moqui.security.UserAccount")
        const username = userId.toLowerCase().replace(/_/g, ".")
        account.setFields({userId, username, userFullName: "Captions " + userId.slice(-1), disabled: "N"}, true, null, null)
        account.create()
    })
}
function endAll() {
    sys(() => {
        for (const s of ec.entity.find(SESSION).condition("startedByUserId", "in", [A, B, C]).condition("statusId", "!=", "CssEnded").list()) {
            s.set("statusId", "CssEnded")
            s.update()
        }
        for (const l of ec.entity.find(LEG).condition("statusId", "in", ["ClsOffered", "ClsAnswered"]).list()) {
            const p = ec.entity.find("moqui.call.CallParticipant").condition("callParticipantId", l.callParticipantId).one()
            if ([A, B, C].includes(p?.userId)) {
                l.set("statusId", "ClsLeft")
                l.update()
            }
        }
    })
}
/** A speech provider for the policy check only: a loopback endpoint nothing here calls. */
function provider(present) {
    sys(() => {
        ec.entity.find("moqui.llm.LlmFeatureBinding").condition({featureId: "transcribe", modelId: MODEL}).deleteAll()
        ec.entity.find("moqui.llm.LlmModel").condition("modelId", MODEL).deleteAll()
        ec.entity.find("moqui.llm.LlmEndpoint").condition("endpointId", ENDPOINT).deleteAll()
        if (!present) return
        if (ec.entity.find("moqui.llm.LlmFeatureBinding").condition("featureId", "transcribe").count() > 0) return
        const rows = [
            ["moqui.llm.LlmEndpoint", {endpointId: ENDPOINT, protocol: "chat-completions", baseUrl: "http://127.0.0.1:9", isLocal: "Y"}],
            ["moqui.llm.LlmModel", {modelId: MODEL, endpointId: ENDPOINT, providerModel: "whisper-test", tier: "fast"}],
            ["moqui.llm.LlmFeatureBinding", {featureId: "transcribe", modelId: MODEL, reasoningLevel: "none"}],
        ]
        for (const [entity, fields] of rows) {
            const row = ec.entity.makeValue(entity)
            row.setFields(fields, true, null, null)
            row.create()
        }
    })
}

let theCall = null

spec({
    stepwise: true,
    setupSpec() {
        for (const id of [A, B, C]) makeUser(id)
        endAll()
        sys(() => call("org.moqui.impl.CallServices.register#MediaNode", {instanceId: NODE, mediaAddress: "127.0.0.1:10000"}))
        const started = as(A, "org.moqui.impl.CallServices.start#Call", {calleeUserIdList: [B], idempotencyKey: "captions-spec-1"})
        as(B, "org.moqui.impl.CallServices.answer#Call", {callSessionId: started.callSessionId})
        theCall = {callSessionId: started.callSessionId}
    },
    cleanupSpec() {
        provider(false)
        endAll()
        sys(() => ec.entity.find("moqui.call.CallMediaNode").condition("instanceId", NODE).deleteAll())
    }
})

feature("with the policy off captions are refused, and the call view has no CC button", {
    when() {
        return withProperty("call_captions_enabled", "false", () => ({
            started: captions(A, "start#Captions", theCall),
            state: captions(A, "get#CaptionState", theCall),
        }))
    },
    then({started, state}) {
        started.reasonCode == "captionsOff"
        started.errors.includes("Captions are turned off")
        state.captions.allowed == false
        state.reasonCode == "captionsOff"
    }
})

feature("someone not in the call is refused with the same parameters", {
    when() {
        return on(() => ({
            started: captions(C, "start#Captions", theCall),
            stopped: captions(C, "stop#Captions", theCall),
            state: captions(C, "get#CaptionState", theCall),
        }))
    },
    then({started, stopped, state}) {
        started.reasonCode == "notFound"
        stopped.reasonCode == "notFound"
        state.reasonCode == "notFound"
        state.captions.allowed == false
    }
})

feature("an end-to-end encrypted call refuses captions", {
    when() {
        sys(() => {
            const row = ec.entity.find(SESSION).condition("callSessionId", theCall.callSessionId).one()
            row.set("encryptionMode", "sframe")
            row.update()
        })
        const out = on(() => ({started: captions(A, "start#Captions", theCall), state: captions(A, "get#CaptionState", theCall)}))
        sys(() => {
            const row = ec.entity.find(SESSION).condition("callSessionId", theCall.callSessionId).one()
            row.set("encryptionMode", "dtls-srtp")
            row.update()
        })
        return out
    },
    then({started, state}) {
        started.reasonCode == "encrypted"
        started.errors.includes("end-to-end encrypted")
        state.captions.allowed == false
        state.reasonCode == "encrypted"
    }
})

feature("someone in the call turns captions on and someone else turns them off", {
    requires: ["llm"],
    when() {
        provider(true)
        return on(() => {
            const started = captions(A, "start#Captions", theCall)
            const again = captions(A, "start#Captions", theCall)
            const seenByB = captions(B, "get#CaptionState", theCall)
            const stopped = captions(B, "stop#Captions", theCall)
            const stoppedAgain = captions(B, "stop#Captions", theCall)
            const row = sys(() => ec.entity.find("moqui.call.CallCaptions").condition("callSessionId", theCall.callSessionId).one())
            return {started, again, seenByB, stopped, stoppedAgain, row}
        })
    },
    then({started, again, seenByB, stopped, stoppedAgain, row}) {
        started.errors == ""
        started.captions.on == true
        started.repeated == false
        again.repeated == true
        seenByB.captions.allowed == true
        seenByB.captions.on == true
        seenByB.captions.startedByUserId == A
        stopped.errors == ""
        stopped.captions.on == false
        stoppedAgain.repeated == true
        row.captionsOn == "N"
        row.startedByUserId == A
        row.stoppedByUserId == B
    }
})

feature("transcribe#Audio answers a reason, not a failure, when there is no audio", {
    when() {
        return sys(() => ec.service.sync().name("org.moqui.impl.SpeechServices.transcribe#Audio")
            .parameters({mediaType: "audio/wav", audio: ""}).call()) || {}
    },
    then(out) {
        out.reasonCode == "invalid"
        out.errorText.includes("audioLocation or audio")
    }
})

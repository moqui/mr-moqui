// The call's actual capture pipeline, including the screenAudio transceiver. No remote SFU is needed to check
// what the presenter publishes. A colored tab and an oscillator distinguish this tab from Chromium's fake monitor.
spec({server: false})

feature("sharing this tab publishes its pixels and audio and releases both tracks on stop", {
    when() {
        const page = browser.newPage({captureThisTab: true})
        page.setContent('<!doctype html><body style="background:rgb(21,185,76)"><button>Share this tab</button></body>')
        for (const src of ["/js/MoquiLib.js", "/js/MoquiL10n.js", "/js/WebrootCall.js"]) page.addScript(src)
        page.evaluate(() => {
            document.querySelector("button").onclick = async () => {
                const call = moqui.call
                const pc = new RTCPeerConnection()
                const sound = new AudioContext()
                const oscillator = sound.createOscillator()
                const gain = sound.createGain()
                gain.gain.value = 0.04
                oscillator.connect(gain).connect(sound.destination)
                try {
                    // Stage signaling is covered in WebrootCall. Install the presenter's grant for this capture.
                    call.pc = pc
                    call.state.current = "test-session"
                    call.state.legId = "presenter"
                    call.state.stage = {kind: "screen", callLegId: "presenter"}
                    const published = []
                    call.send = msg => { if (msg.rtc === "publish") published.push(msg.source) }
                    await sound.resume()
                    oscillator.start()
                    if (!await call.shareScreen("tab", false)) throw Error(call.state.error || "capture failed")
                    const stream = call.screenStream
                    const video = document.createElement("video")
                    video.muted = true
                    video.srcObject = stream
                    await video.play()
                    const analyser = sound.createAnalyser()
                    sound.createMediaStreamSource(stream).connect(analyser)
                    const canvas = document.createElement("canvas")
                    canvas.width = 32; canvas.height = 24
                    const context = canvas.getContext("2d")
                    const samples = new Float32Array(analyser.fftSize)
                    const until = performance.now() + 4000
                    let green = false, audible = false
                    while ((!green || !audible) && performance.now() < until) {
                        await new Promise(resolve => setTimeout(resolve, 50))
                        context.drawImage(video, 0, 0, 32, 24)
                        const [r,g,b] = context.getImageData(16,12,1,1).data
                        // RGB to YUV conversion during capture shifts individual channels.
                        green = Math.abs(r - 21) < 32 && Math.abs(g - 185) < 32 && Math.abs(b - 76) < 32
                        analyser.getFloatTimeDomainData(samples)
                        audible = samples.some(sample => Math.abs(sample) > 0.005)
                    }
                    const tracks = stream.getTracks()
                    const surface = stream.getVideoTracks()[0].getSettings().displaySurface
                    await call.stopShare()
                    window.captureResult = {green, audible, published, surface, stopped: tracks.every(t => t.readyState === "ended")}
                } catch (error) { window.captureResult = {error: String(error)} }
                finally {
                    pc.close()
                    await sound.close()
                }
            }
            return true
        })
        page.getByRole("button", {name: "Share this tab"}).click()
        const result = page.waitFor(() => window.captureResult)
        if (result.error) throw Error(result.error)
        return result
    },
    then({green, audible, published, surface, stopped, error}) {
        !error
        surface == "browser"
        green
        audible
        published.includes("screen") && published.includes("screenAudio")
        stopped
    }
})

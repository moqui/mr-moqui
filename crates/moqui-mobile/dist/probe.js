// Polls the Rust side for the outcome of moqui-client's request (src/lib.rs, probe_result).
const out = document.getElementById('probe');
async function poll() {
    const text = await window.__TAURI__.core.invoke('probe_result');
    if (text) {
        out.textContent = text;
    } else {
        setTimeout(poll, 300);
    }
}
poll().catch((err) => {
    out.textContent = 'invoke failed: ' + err;
});

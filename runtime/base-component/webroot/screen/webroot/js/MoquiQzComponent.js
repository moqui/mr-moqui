/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

/*
 * Printing through QZ Tray 2.3: the print options dialog of the header, and the functions screens call to print a URL.
 * WebrootVue.qvt.js loads qz-tray.js and then this file only when the user preference qz.print.enabled is true (the
 * shell settings say so). QZ 2.3 uses native promises, so there is no RSVP and no SHA-256 script. This is the Java
 * MoquiQzComponent.js on QDialog and QBtn, without Bootstrap markup.
 *
 * To sign requests the server has a key and the page serves the certificate:
 *   openssl req -x509 -newkey rsa:2048 -keyout qz-private-key.pem -out digital-certificate.txt -days 11499 -nodes
 *   (the Common Name is the host name, localhost works for local testing)
 * Put qz-private-key.pem in the classes directory of the runtime or a component, add authcert.override=<path of the certificate> to
 * qz-tray.properties, and serve the certificate at /qz-tray/digital-certificate.txt. The signature is the qzSign transition of
 * apps.xml. Without a certificate QZ asks the user to trust each request.
 */
(function (root) {
    "use strict";

    const moqui = root.moqui;
    const qz = root.qz;
    if (!qz || !moqui || !moqui.qvt) {
        console.info("Not creating the QZ component, qz: " + qz + ", moqui: " + moqui);
        return;
    }

    const orientations = [{ value: "", label: "Default" }, { value: "portrait", label: "Portrait" }, { value: "landscape", label: "Landscape" }, { value: "reverse-landscape", label: "Reverse LS" }];
    const colorTypes = [{ value: "", label: "Default" }, { value: "color", label: "Color" }, { value: "grayscale", label: "Grayscale" }, { value: "blackwhite", label: "Black & White" }];

    const options = (key, title) =>
        '<div class="col-xs-12 col-sm-6 q-pa-sm"><h6>' + title + '</h6>' +
            '<q-select dense outlined emit-value map-options stack-label label="Orientation" v-model="' + key + '.orientation" :options="orientations" class="q-my-sm"></q-select>' +
            '<q-select dense outlined emit-value map-options stack-label label="Color Type" v-model="' + key + '.colorType" :options="colorTypes" class="q-my-sm"></q-select>' +
            '<q-checkbox dense v-model="' + key + '.scaleContent" label="Scale Content"></q-checkbox><br>' +
            '<q-checkbox dense v-model="' + key + '.rasterize" label="Rasterize"></q-checkbox>' +
        '</div>';

    const component = {
        name: "MoquiQz",
        data() {
            return {
                show: false, connectionState: "Inactive", connectionClass: "text-grey", qzVersion: null, currentPrinter: null, currentLabelPrinter: null, printerList: [],
                mainOptions: { orientation: "", colorType: "color", scaleContent: true, rasterize: false },
                labelOptions: { orientation: "", colorType: "blackwhite", scaleContent: false, rasterize: false }, orientations, colorTypes
            };
        },
        template:
            '<span>' +
                '<q-btn dense flat icon="print" :color="readyColor" aria-label="Print options" @click="show = true"><q-tooltip>Print Options</q-tooltip></q-btn>' +
                '<m-dialog v-model="show" title="QZ Print Options" width="720">' +
                    '<div class="row items-center">' +
                        '<div class="col"><h6 :class="connectionClass">Connection: {{connectionState}}<span v-if="qzVersion" class="text-grey"> ver {{qzVersion}}</span></h6></div>' +
                        '<div class="col-auto">' +
                            '<q-btn v-if="connectionState !== \'Active\'" dense outline no-caps color="positive" label="Connect" @click="startConnection()"></q-btn>' +
                            '<q-btn v-if="connectionState === \'Active\'" dense outline no-caps color="warning" label="Disconnect" @click="endConnection()"></q-btn>' +
                            '<q-btn v-if="connectionState === \'Inactive\' || connectionState === \'Error\'" dense outline no-caps color="info" label="Launch QZ" @click="launchQZ()"></q-btn>' +
                        '</div>' +
                    '</div>' +
                    '<p v-if="connectionState !== \'Active\'">Do not have QZ Tray installed? <a href="https://qz.io/download/" target="_blank" rel="noopener">Download from QZ.io</a></p>' +
                    '<q-card flat bordered class="q-my-sm"><q-card-actions>' +
                        '<span>Printers: Main <strong class="text-info">{{currentPrinter||"not set"}}</strong>' +
                            '<q-btn v-if="currentPrinter" dense flat icon="close" aria-label="Clear the main printer" @click="setPrinter(null)"></q-btn></span>' +
                        '<span class="q-ml-md">Label <strong class="text-info">{{currentLabelPrinter||"not set"}}</strong>' +
                            '<q-btn v-if="currentLabelPrinter" dense flat icon="close" aria-label="Clear the label printer" @click="setLabelPrinter(null)"></q-btn></span>' +
                        '<q-space></q-space><q-btn dense outline no-caps label="Find" @click="findPrinters()"></q-btn>' +
                    '</q-card-actions><q-card-section class="row">' +
                        '<div v-for="printerName in printerList" :key="printerName" class="col-xs-12 col-sm-6 q-py-xs">{{printerName}} ' +
                            '<q-btn dense no-caps :outline="currentPrinter !== printerName" :color="currentPrinter === printerName ? \'primary\' : \'\'" label="Main" @click="setPrinter(currentPrinter === printerName ? null : printerName)"></q-btn>' +
                            '<q-btn dense no-caps :outline="currentLabelPrinter !== printerName" :color="currentLabelPrinter === printerName ? \'primary\' : \'\'" label="Label" @click="setLabelPrinter(currentLabelPrinter === printerName ? null : printerName)"></q-btn>' +
                        '</div>' +
                    '</q-card-section></q-card>' +
                    '<div class="row">' + options("mainOptions", "Main Options") + options("labelOptions", "Label Options") + '</div>' +
                '</m-dialog>' +
            '</span>',
        computed: {
            readyColor() {
                if (this.connectionState !== "Active") return "negative";
                return this.currentPrinter ? (this.currentLabelPrinter ? "positive" : "primary") : (this.currentLabelPrinter ? "info" : "warning");
            }
        },
        methods: {
            findPrinters() {
                qz.printers.find().then((data) => {
                    if (!Array.isArray(data)) return;
                    const labelKeywords = ["label", "zebra", "sato", "dymo"];
                    this.printerList = data;
                    data.forEach((printerName) => {
                        const hasLabelKw = labelKeywords.some((word) => printerName.toLowerCase().indexOf(word) !== -1);
                        if (!hasLabelKw && !this.currentPrinter) this.setPrinter(printerName);
                        if (hasLabelKw && !this.currentLabelPrinter) this.setLabelPrinter(printerName);
                    });
                }).catch(moqui.showQzError);
            },
            savePreference(key, value) {
                moqui.request(this.$root.appRootPath + "/apps/setPreference", { method: "POST", params: { moquiSessionToken: this.$root.moquiSessionToken, preferenceKey: key, preferenceValue: value } })
                    .catch(moqui.handleAjaxError);
            },
            setPrinter(printerName) { this.currentPrinter = printerName; this.savePreference("qz.printer.main.active", printerName); },
            setLabelPrinter(printerName) { this.currentLabelPrinter = printerName; this.savePreference("qz.printer.label.active", printerName); },
            handleConnectionError(err) {
                this.connectionState = "Error";
                this.connectionClass = "text-negative";
                if (err && err.target !== undefined) {
                    if (err.target.readyState >= 2) moqui.notifyGrowl({ type: "info", title: "Connection to QZ Tray was closed" });
                    else {
                        moqui.notifyGrowl({ type: "danger", title: "A QZ Tray connection error occurred, check log for details" });
                        console.error(err);
                    }
                } else moqui.notifyGrowl({ type: "danger", title: String(err) });
            },
            launchQZ() {
                if (!qz.websocket.isActive()) {
                    root.location.assign("qz:launch");
                    this.startConnection();
                }
            },
            startConnection() {
                if (qz.websocket.isActive()) {
                    this.connectionState = "Active";
                    this.connectionClass = "text-positive";
                    return;
                }
                this.connectionState = "Connecting...";
                this.connectionClass = "text-info";
                // five attempts, a second apart
                qz.websocket.connect({ retries: 5, delay: 1 }).then(() => {
                    this.connectionState = "Active";
                    this.connectionClass = "text-positive";
                    qz.api.getVersion().then((data) => { this.qzVersion = data; }).catch(moqui.showQzError);
                }).catch(this.handleConnectionError);
            },
            endConnection() {
                if (!qz.websocket.isActive()) {
                    this.connectionState = "Inactive";
                    this.connectionClass = "text-grey";
                    return;
                }
                qz.websocket.disconnect().then(() => {
                    this.connectionState = "Inactive";
                    this.connectionClass = "text-grey";
                    this.qzVersion = null;
                }).catch(this.handleConnectionError);
            }
        },
        mounted() {
            this.startConnection();
            const rootPath = this.$root.appRootPath;
            qz.security.setCertificatePromise((resolve, reject) => {
                root.fetch(rootPath + "/qz-tray/digital-certificate.txt", { credentials: "same-origin" })
                    .then((response) => (response.ok ? response.text() : Promise.reject(new Error("no certificate")))).then(resolve, reject);
            });
            qz.security.setSignaturePromise((toSign) => (resolve) => {
                // resolve with nothing when signing fails, so that QZ asks the user and printing is still possible
                root.fetch(rootPath + "/apps/qzSign?message=" + encodeURIComponent(toSign), { credentials: "same-origin" })
                    .then((response) => (response.ok ? response.text() : "")).then(resolve, () => resolve());
            });
            moqui.request(rootPath + "/apps/getPreferences", { params: { keyRegexp: "qz\\.print.*" } }).then((result) => {
                const prefs = result.json || {};
                if (prefs["qz.printer.main.active"]) this.currentPrinter = prefs["qz.printer.main.active"];
                if (prefs["qz.printer.label.active"]) this.currentLabelPrinter = prefs["qz.printer.label.active"];
            }, moqui.handleAjaxError);
        }
    };
    moqui.qzComponent = component;

    moqui.isQzActive = () => qz.websocket.isActive();
    // useful options: jobName, copies, scaleContent, rasterize, rotation; see https://qz.io/wiki/2.0-Pixel-Printing#advanced-printing
    moqui.getQzConfig = (printer, userOptions, jobOptions) => {
        const cfgOptions = Object.assign({ copies: 1, jobName: null }, moqui.isPlainObject(userOptions) ? userOptions : null, moqui.isPlainObject(jobOptions) ? jobOptions : null);
        const cfg = qz.configs.create(printer, cfgOptions);
        cfg.reconfigure(cfgOptions);
        return cfg;
    };
    const qzVm = () => moqui.webrootVue.$refs.qzVue;
    moqui.getQzMainConfig = (jobOptions) => moqui.getQzConfig(qzVm().currentPrinter, qzVm().mainOptions, jobOptions);
    moqui.getQzLabelConfig = (jobOptions) => moqui.getQzConfig(qzVm().currentLabelPrinter, qzVm().labelOptions, jobOptions);
    moqui.showQzError = (error) => { console.log(error); moqui.notifyGrowl({ type: "danger", title: String(error) }); };

    // The files are fetched here, not by QZ Tray: its request would be in no session and would not be authorized.
    moqui.internalChainPrintFile = (config, type, urlArray, urlIndex) => {
        if (urlIndex >= urlArray.length) return;
        root.fetch(urlArray[urlIndex], { credentials: "same-origin" }).then((response) => response.blob()).then((blob) => new Promise((resolve) => {
            const reader = new root.FileReader();
            reader.onloadend = () => resolve(reader.result);
            reader.readAsDataURL(blob);
        })).then((dataUrl) => {
            const printData = { type, format: "base64", data: dataUrl.substr(dataUrl.indexOf(",") + 1) };
            return qz.print(config, [printData]).catch(moqui.showQzError);
        }).then(() => moqui.internalChainPrintFile(config, type, urlArray, urlIndex + 1), moqui.showQzError);
    };
    moqui.printUrlFile = (config, url, type) => {
        if (!url || !url.length) { console.warn("Called printUrlFile with no url array or string, printing nothing"); return; }
        if (!moqui.isQzActive()) {
            moqui.notifyGrowl({ type: "warning", title: "Cannot print, QZ is not active" });
            return;
        }
        moqui.internalChainPrintFile(config, type && type.length ? type : "pdf", Array.isArray(url) ? url : [url], 0);
    };
    moqui.printUrlMain = (url, type) => moqui.printUrlFile(moqui.getQzMainConfig(), url, type);
    moqui.printUrlLabel = (url, type) => moqui.printUrlFile(moqui.getQzLabelConfig(), url, type);
})(globalThis);

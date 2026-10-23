if (sri.doBoundaryComments()) sri.write("<!-- BEGIN screen[@location=" + sri.getActiveScreenDef().location + "].widgets -->\n");
if (sri.doBoundaryComments()) sri.write("<!-- BEGIN form-single[@name=LinkForm] -->\n");
sri.pushSingleFormMapContext(fieldValues);
try {
    $h.formOpen({"name":"form-single","attrs":{"name":"LinkForm"}}, false, ["a","b","c","d","e"]);
    {
        const $title = ec.l10n.localize("A");
        const $open = $h.fieldOpen("a", $title, false, "", false);
        sri.pushContext();
        try {
            $h.link({"name":"link","attrs":{"url":"go","text":"Go ${a}","link-type":"anchor"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"a"},"parent":{"name":"form-single","attrs":{"name":"LinkForm"}}}}}, true);
        } finally {
            sri.popContext();
        }
        $h.fieldClose($open, false);
    }
    {
        const $title = ec.l10n.localize("B");
        const $open = $h.fieldOpen("b", $title, false, "", false);
        sri.pushContext();
        try {
            $h.link({"name":"link","attrs":{"url":"go","text":"Hidden ${b}","link-type":"hidden-form-link","condition":"b != null"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"b"},"parent":{"name":"form-single","attrs":{"name":"LinkForm"}}}}}, true);
        } finally {
            sri.popContext();
        }
        $h.fieldClose($open, false);
    }
    {
        const $title = ec.l10n.localize("C");
        const $open = $h.fieldOpen("c", $title, false, "", false);
        sri.pushContext();
        try {
            $h.link({"name":"link","attrs":{"url":"go","text":"Plain","link-type":"auto"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"c"},"parent":{"name":"form-single","attrs":{"name":"LinkForm"}}}}}, true);
        } finally {
            sri.popContext();
        }
        $h.fieldClose($open, false);
    }
    {
        const $title = ec.l10n.localize("D");
        const $open = $h.fieldOpen("d", $title, false, "", false);
        sri.pushContext();
        try {
            $h.link({"name":"link","attrs":{"url":"go","entity-name":"moqui.basic.Enumeration","link-type":"anchor","text":"${description}"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"d"},"parent":{"name":"form-single","attrs":{"name":"LinkForm"}}}}}, true);
        } finally {
            sri.popContext();
        }
        $h.fieldClose($open, false);
    }
    {
        const $title = ec.l10n.localize("E");
        const $open = $h.fieldOpen("e", $title, false, "", false);
        sri.pushContext();
        try {
            $h.link({"name":"link","attrs":{"url":"go","text":"Mapped ${v}","text-map":"{v: 1}","link-type":"anchor"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"e"},"parent":{"name":"form-single","attrs":{"name":"LinkForm"}}}}}, true);
        } finally {
            sri.popContext();
        }
        $h.fieldClose($open, false);
    }
    $h.formClose({"name":"form-single","attrs":{"name":"LinkForm"}});
} finally {
    sri.popContext();
}
if (sri.doBoundaryComments()) sri.write("<!-- END   form-single[@name=LinkForm] -->\n");
$h.link({"name":"link","attrs":{"url":"go","text":"Standalone"}}, false);
$h.label({"name":"label","attrs":{"text":"Cond ${name}","condition":"name"}});
$h.label({"name":"label","attrs":{"text":"Mapped ${v}","text-map":"{v: 2}"}});
$h.image({"name":"image","attrs":{"url":"/img.png","alt":"An image"}});
if (sri.doBoundaryComments()) sri.write("<!-- END   screen[@location=" + sri.getActiveScreenDef().location + "].widgets -->\n");

$lineChars = Number(lineCharacters || "132");
$pageLines = pageLines ? Number(pageLines) : 0;
$lineWrap = lineWrap === "true";
sri.pushSingleFormMapContext(fieldValues);
try {
    $cellWidth = $lineChars * 0.8;
    sri.write($pad(ec.l10n.localize("A"), $lineChars * 0.2, true) + ": ");
    {
        let $linkText = ec.resource.expand("Go ${a}");
        if ($linkText === "null") $linkText = "";
        sri.write($pad($linkText));
    }
    sri.write("\n");
    $cellWidth = $lineChars * 0.8;
    sri.write($pad(ec.l10n.localize("B"), $lineChars * 0.2, true) + ": ");
    if (b != null) {
        {
            let $linkText = ec.resource.expand("Hidden ${b}");
            if ($linkText === "null") $linkText = "";
            sri.write($pad($linkText));
        }
    }
    sri.write("\n");
    $cellWidth = $lineChars * 0.8;
    sri.write($pad(ec.l10n.localize("C"), $lineChars * 0.2, true) + ": ");
    {
        let $linkText = ec.resource.expand("Plain");
        if ($linkText === "null") $linkText = "";
        sri.write($pad($linkText));
    }
    sri.write("\n");
    $cellWidth = $lineChars * 0.8;
    sri.write($pad(ec.l10n.localize("D"), $lineChars * 0.2, true) + ": ");
    {
        let $linkText = sri.getFieldEntityValue({"name":"link","attrs":{"url":"go","entity-name":"moqui.basic.Enumeration","link-type":"anchor","text":"${description}"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"d"},"parent":{"name":"form-single","attrs":{"name":"LinkForm"},"parent":{"name":"widgets","attrs":{}}}}}});
        if ($linkText === "null") $linkText = "";
        sri.write($pad($linkText));
    }
    sri.write("\n");
    $cellWidth = $lineChars * 0.8;
    sri.write($pad(ec.l10n.localize("E"), $lineChars * 0.2, true) + ": ");
    {
        let $linkText = ec.resource.expand("Mapped ${v}", "", {v: 1});
        if ($linkText === "null") $linkText = "";
        sri.write($pad($linkText));
    }
    sri.write("\n");
} finally {
    sri.popContext();
}
$cellWidth = 0;
sri.write("\n");
sri.write($pad(ec.resource.expand("Cond ${name}")));
sri.write($pad(ec.resource.expand("Mapped ${v}", "", {v: 2})));
sri.write($pad("An image"));

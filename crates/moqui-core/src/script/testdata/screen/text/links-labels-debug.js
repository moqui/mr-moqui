// <widgets>
$lineChars = Number(lineCharacters || "132");
$pageLines = pageLines ? Number(pageLines) : 0;
$lineWrap = lineWrap === "true";
// <form-single name="LinkForm">
sri.pushSingleFormMapContext(fieldValues);
try {
    $cellWidth = $lineChars * 0.8;
    sri.write($pad(ec.l10n.localize("A"), $lineChars * 0.2, true) + ": ");
    // <link url="go" text="Go ${a}" link-type="anchor">
    {
        let $linkText = ec.resource.expand("Go ${a}");
        if ($linkText === "null") $linkText = "";
        sri.write($pad($linkText));
    }
    sri.write("\n");
    $cellWidth = $lineChars * 0.8;
    sri.write($pad(ec.l10n.localize("B"), $lineChars * 0.2, true) + ": ");
    // <link url="go" text="Hidden ${b}" link-type="hidden-form-link" condition="b != null">
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
    // <link url="go" text="Plain" link-type="auto">
    {
        let $linkText = ec.resource.expand("Plain");
        if ($linkText === "null") $linkText = "";
        sri.write($pad($linkText));
    }
    sri.write("\n");
    $cellWidth = $lineChars * 0.8;
    sri.write($pad(ec.l10n.localize("D"), $lineChars * 0.2, true) + ": ");
    // <link url="go" entity-name="moqui.basic.Enumeration" link-type="anchor" text="${description}">
    {
        let $linkText = sri.getFieldEntityValue({"name":"link","attrs":{"url":"go","entity-name":"moqui.basic.Enumeration","link-type":"anchor","text":"${description}"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"d"},"parent":{"name":"form-single","attrs":{"name":"LinkForm"},"parent":{"name":"widgets","attrs":{}}}}}});
        if ($linkText === "null") $linkText = "";
        sri.write($pad($linkText));
    }
    sri.write("\n");
    $cellWidth = $lineChars * 0.8;
    sri.write($pad(ec.l10n.localize("E"), $lineChars * 0.2, true) + ": ");
    // <link url="go" text="Mapped ${v}" text-map="{v: 1}" link-type="anchor">
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
// <label text="Cond ${name}" condition="name">
sri.write($pad(ec.resource.expand("Cond ${name}")));
// <label text="Mapped ${v}" text-map="{v: 2}">
sri.write($pad(ec.resource.expand("Mapped ${v}", "", {v: 2})));
// <image url="/img.png" alt="An image">
sri.write($pad("An image"));

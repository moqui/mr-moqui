if (sri.doBoundaryComments()) sri.write("<!-- BEGIN screen[@location=" + sri.getActiveScreenDef().location + "].widgets -->\n");
if (sri.doBoundaryComments()) sri.write("<!-- BEGIN form-single[@name=Everything] -->\n");
sri.pushSingleFormMapContext(fv);
try {
    $h.formOpen({"name":"form-single","attrs":{"name":"Everything","map":"fv","background-submit":"true","background-reload-id":"Dyn","focus-field":"firstName","transition":"save","pass-through-parameters":"true"}}, true, ["firstName","email","ac","dflt","bio","find","status","multi","remote","kind","flag","stamp","day","at","between","period","range","amount","shown","dynamicShown","enumId","doc","pw","where","clear","secret","skip","hid","mix","go","viaLink"]);
    $h.fieldGroup({"name":"field-group","attrs":{"title":"Names","box":"true"}});
    {
        const $title = ec.l10n.localize("First");
        const $open = $h.fieldOpen("firstName", $title, false, "", false);
        sri.pushContext();
        try {
            $h.textLine({"name":"text-line","attrs":{"size":"20","maxlength":"40","mask":"999"},"parent":{"name":"default-field","attrs":{"title":"First"},"parent":{"name":"field","attrs":{"name":"firstName"},"parent":{"name":"form-single","attrs":{"name":"Everything","map":"fv"}}}}});
        } finally {
            sri.popContext();
        }
        $h.fieldClose($open, false);
    }
    {
        const $title = ec.l10n.localize("Email");
        const $open = $h.fieldOpen("email", $title, false, "", false);
        sri.pushContext();
        try {
            $h.textLine({"name":"text-line","attrs":{"input-type":"email","validate-service":"x.y.Z","validate-parameter":"mail","disabled":"false"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"email"},"parent":{"name":"form-single","attrs":{"name":"Everything","map":"fv"}}}}});
        } finally {
            sri.popContext();
        }
        $h.fieldClose($open, false);
    }
    $h.fieldGroupClose({"name":"field-group","attrs":{"title":"Names","box":"true"}});
    $h.accordion("Acc", "2");
    sri.write("<div class=\"accordion\">");
    $h.fieldGroup({"name":"field-group","attrs":{"title":"One"}});
    {
        const $title = ec.l10n.localize("Ac");
        const $open = $h.fieldOpen("ac", $title, false, "", false);
        sri.pushContext();
        try {
            $h.textLine({"name":"text-line","attrs":{"ac-transition":"find","ac-min-length":"2","ac-show-value":"true","ac-use-actual":"true"},"children":[{"name":"depends-on","attrs":{"field":"firstName","parameter":"fn"}}],"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"ac"},"parent":{"name":"form-single","attrs":{"name":"Everything","map":"fv"}}}}});
        } finally {
            sri.popContext();
        }
        $h.fieldClose($open, false);
    }
    $h.fieldGroupClose({"name":"field-group","attrs":{"title":"One"}});
    $h.fieldGroup({"name":"field-group","attrs":{"title":"Two"}});
    {
        const $title = ec.l10n.localize("Dflt");
        const $open = $h.fieldOpen("dflt", $title, false, "", false);
        sri.pushContext();
        try {
            $h.textLine({"name":"text-line","attrs":{"default-transition":"fill"},"children":[{"name":"depends-on","attrs":{"field":"firstName"}}],"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"dflt"},"parent":{"name":"form-single","attrs":{"name":"Everything","map":"fv"}}}}});
        } finally {
            sri.popContext();
        }
        $h.fieldClose($open, false);
    }
    $h.fieldGroupClose({"name":"field-group","attrs":{"title":"Two"}});
    sri.write("</div>\n");
    $h.endAccordion();
    $h.rowOpen("field-row", "");
    sri.write("<div>");
    {
        const $title = ec.l10n.localize("Bio");
        const $open = $h.fieldOpen("bio", $title, false, "", false);
        sri.pushContext();
        try {
            $h.textArea({"name":"text-area","attrs":{"rows":"4","editor-type":"html"},"parent":{"name":"default-field","attrs":{"tooltip":"About you"},"parent":{"name":"field","attrs":{"name":"bio"},"parent":{"name":"form-single","attrs":{"name":"Everything","map":"fv"}}}}});
        } finally {
            sri.popContext();
        }
        $h.fieldClose($open, false);
    }
    sri.write("</div>");
    sri.write("<div>");
    {
        const $title = ec.l10n.localize("Find");
        const $open = $h.fieldOpen("find", $title, false, "", false);
        sri.pushContext();
        try {
            $h.textFind({"name":"text-find","attrs":{"default-operator":"begins"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"find"},"parent":{"name":"form-single","attrs":{"name":"Everything","map":"fv"}}}}});
        } finally {
            sri.popContext();
        }
        $h.fieldClose($open, false);
    }
    sri.write("</div>");
    sri.write("</div>\n");
    sri.write("<div class=\"field big-row\">");
    sri.write("<label>" + ec.resource.expand("Big", "") + "</label>");
    sri.write("<div class=\"widget\">");
    {
        const $title = ec.l10n.localize("Status");
        const $open = $h.fieldOpen("status", $title, false, "", true);
        sri.pushContext();
        try {
            $h.dropDown({"name":"drop-down","attrs":{"allow-empty":"true","show-not":"true","current-description":"Gone"},"children":[{"name":"option","attrs":{"key":"A","text":"Active"}},{"name":"option","attrs":{"key":"${k}"}}],"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"status"},"parent":{"name":"form-single","attrs":{"name":"Everything","map":"fv"}}}}});
        } finally {
            sri.popContext();
        }
        $h.fieldClose($open, true);
    }
    {
        const $title = ec.l10n.localize("Multi");
        const $open = $h.fieldOpen("multi", $title, false, "", true);
        sri.pushContext();
        try {
            $h.dropDown({"name":"drop-down","attrs":{"allow-multiple":"true","combo-box":"true"},"children":[{"name":"option","attrs":{"key":"A"}},{"name":"option","attrs":{"key":"B"}}],"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"multi"},"parent":{"name":"form-single","attrs":{"name":"Everything","map":"fv"}}}}});
        } finally {
            sri.popContext();
        }
        $h.fieldClose($open, true);
    }
    sri.write("</div></div>\n");
    $h.rowOpen("row", "cols");
    $h.colOpen({"name":"field-col","attrs":{"md":"6","label-cols":"3"}});
    {
        const $title = ec.l10n.localize("Remote");
        const $open = $h.fieldOpen("remote", $title, false, "", false);
        sri.pushContext();
        try {
            $h.dropDown({"name":"drop-down","attrs":{"allow-empty":"true"},"children":[{"name":"dynamic-options","attrs":{"transition":"options","server-search":"true","min-length":"2","label-field":"text","value-field":"id"},"children":[{"name":"depends-on","attrs":{"field":"status","parameter":"st"}}]}],"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"remote"},"parent":{"name":"form-single","attrs":{"name":"Everything","map":"fv"}}}}});
        } finally {
            sri.popContext();
        }
        $h.fieldClose($open, false);
    }
    sri.write("</div>");
    $h.colOpen({"name":"field-col","attrs":{"md":"6"}});
    {
        const $title = ec.l10n.localize("Kind");
        const $open = $h.fieldOpen("kind", $title, false, "", false);
        sri.pushContext();
        try {
            $h.radio({"name":"radio","attrs":{"no-current-selected-key":"X"},"children":[{"name":"option","attrs":{"key":"X"}},{"name":"option","attrs":{"key":"Y","text":"Why"}}],"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"kind"},"parent":{"name":"form-single","attrs":{"name":"Everything","map":"fv"}}}}});
        } finally {
            sri.popContext();
        }
        $h.fieldClose($open, false);
    }
    sri.write("</div>");
    sri.write("</div>\n");
    {
        const $title = ec.l10n.localize("Flag");
        const $open = $h.fieldOpen("flag", $title, false, "", false);
        sri.pushContext();
        try {
            $h.check({"name":"check","attrs":{"all-checked":"false"},"children":[{"name":"option","attrs":{"key":"Y","text":"Yes"}}],"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"flag"},"parent":{"name":"form-single","attrs":{"name":"Everything","map":"fv"}}}}});
        } finally {
            sri.popContext();
        }
        $h.fieldClose($open, false);
    }
    {
        const $title = ec.l10n.localize("Stamp");
        const $open = $h.fieldOpen("stamp", $title, false, "", false);
        sri.pushContext();
        try {
            $h.dateTime({"name":"date-time","attrs":{"minute-stepping":"10"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"stamp"},"parent":{"name":"form-single","attrs":{"name":"Everything","map":"fv"}}}}});
        } finally {
            sri.popContext();
        }
        $h.fieldClose($open, false);
    }
    {
        const $title = ec.l10n.localize("Day");
        const $open = $h.fieldOpen("day", $title, false, "", false);
        sri.pushContext();
        try {
            $h.dateTime({"name":"date-time","attrs":{"type":"date","format":"MM/dd/yyyy"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"day"},"parent":{"name":"form-single","attrs":{"name":"Everything","map":"fv"}}}}});
        } finally {
            sri.popContext();
        }
        $h.fieldClose($open, false);
    }
    {
        const $title = ec.l10n.localize("At");
        const $open = $h.fieldOpen("at", $title, false, "", false);
        sri.pushContext();
        try {
            $h.dateTime({"name":"date-time","attrs":{"type":"time","default-value":"12:00"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"at"},"parent":{"name":"form-single","attrs":{"name":"Everything","map":"fv"}}}}});
        } finally {
            sri.popContext();
        }
        $h.fieldClose($open, false);
    }
    {
        const $title = ec.l10n.localize("Between");
        const $open = $h.fieldOpen("between", $title, false, "", false);
        sri.pushContext();
        try {
            $h.dateFind({"name":"date-find","attrs":{"type":"date"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"between"},"parent":{"name":"form-single","attrs":{"name":"Everything","map":"fv"}}}}});
        } finally {
            sri.popContext();
        }
        $h.fieldClose($open, false);
    }
    {
        const $title = ec.l10n.localize("Period");
        const $open = $h.fieldOpen("period", $title, false, "", false);
        sri.pushContext();
        try {
            $h.datePeriod({"name":"date-period","attrs":{"allow-empty":"false"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"period"},"parent":{"name":"form-single","attrs":{"name":"Everything","map":"fv"}}}}});
        } finally {
            sri.popContext();
        }
        $h.fieldClose($open, false);
    }
    {
        const $title = ec.l10n.localize("Range");
        const $open = $h.fieldOpen("range", $title, false, "", false);
        sri.pushContext();
        try {
            $h.rangeFind({"name":"range-find","attrs":{"size":"6"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"range"},"parent":{"name":"form-single","attrs":{"name":"Everything","map":"fv"}}}}});
        } finally {
            sri.popContext();
        }
        $h.fieldClose($open, false);
    }
    {
        const $title = ec.l10n.localize("Amount");
        const $open = $h.fieldOpen("amount", $title, false, "", false);
        sri.pushContext();
        try {
            $h.display({"name":"display","attrs":{"currency-unit-field":"uom","style":"big"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"amount"},"parent":{"name":"form-single","attrs":{"name":"Everything","map":"fv"}}}}});
        } finally {
            sri.popContext();
        }
        $h.fieldClose($open, false);
    }
    {
        const $title = ec.l10n.localize("Shown");
        const $open = $h.fieldOpen("shown", $title, false, "", false);
        sri.pushContext();
        try {
            $h.display({"name":"display","attrs":{"text":"Total ${total}","also-hidden":"false"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"shown"},"parent":{"name":"form-single","attrs":{"name":"Everything","map":"fv"}}}}});
        } finally {
            sri.popContext();
        }
        $h.fieldClose($open, false);
    }
    {
        const $title = ec.l10n.localize("Dynamic Shown");
        const $open = $h.fieldOpen("dynamicShown", $title, false, "", false);
        sri.pushContext();
        try {
            $h.display({"name":"display","attrs":{"dynamic-transition":"lookup"},"children":[{"name":"depends-on","attrs":{"field":"status","parameter":"st"}}],"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"dynamicShown"},"parent":{"name":"form-single","attrs":{"name":"Everything","map":"fv"}}}}});
        } finally {
            sri.popContext();
        }
        $h.fieldClose($open, false);
    }
    {
        const $title = ec.l10n.localize("Enum ID");
        const $open = $h.fieldOpen("enumId", $title, false, "", false);
        sri.pushContext();
        try {
            $h.displayEntity({"name":"display-entity","attrs":{"entity-name":"moqui.basic.Enumeration","text":"${description}"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"enumId"},"parent":{"name":"form-single","attrs":{"name":"Everything","map":"fv"}}}}});
        } finally {
            sri.popContext();
        }
        $h.fieldClose($open, false);
    }
    {
        const $title = ec.l10n.localize("Doc");
        const $open = $h.fieldOpen("doc", $title, false, "", false);
        sri.pushContext();
        try {
            $h.file({"name":"file","attrs":{"accept":"image/*","multiple":"true"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"doc"},"parent":{"name":"form-single","attrs":{"name":"Everything","map":"fv"}}}}});
        } finally {
            sri.popContext();
        }
        $h.fieldClose($open, false);
    }
    {
        const $title = ec.l10n.localize("Pw");
        const $open = $h.fieldOpen("pw", $title, false, "", false);
        sri.pushContext();
        try {
            $h.password({"name":"password","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"pw"},"parent":{"name":"form-single","attrs":{"name":"Everything","map":"fv"}}}}});
        } finally {
            sri.popContext();
        }
        $h.fieldClose($open, false);
    }
    {
        const $title = ec.l10n.localize("Where");
        const $open = $h.fieldOpen("where", $title, false, "", false);
        sri.pushContext();
        try {
            $h.textLine({"name":"lookup","attrs":{"size":"12"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"where"},"parent":{"name":"form-single","attrs":{"name":"Everything","map":"fv"}}}}});
        } finally {
            sri.popContext();
        }
        $h.fieldClose($open, false);
    }
    {
        const $title = ec.l10n.localize("Clear");
        const $open = $h.fieldOpen("clear", $title, false, "", false);
        sri.pushContext();
        try {
            $h.reset_({"name":"reset","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"clear"},"parent":{"name":"form-single","attrs":{"name":"Everything","map":"fv"}}}}}, ec.l10n.localize("Clear"));
        } finally {
            sri.popContext();
        }
        $h.fieldClose($open, false);
    }
    $h.hidden({"name":"hidden","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"secret"},"parent":{"name":"form-single","attrs":{"name":"Everything","map":"fv"}}}}});
    if (!ec.resource.condition("true", "")) {
        {
            const $title = ec.l10n.localize("Hid");
            const $open = $h.fieldOpen("hid", $title, false, "", false);
            sri.pushContext();
            try {
                $h.display({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"hid","hide":"true"},"parent":{"name":"form-single","attrs":{"name":"Everything","map":"fv"}}}}});
            } finally {
                sri.popContext();
            }
            $h.fieldClose($open, false);
        }
    }
    if (mode == 'a') {
        {
            const $title = ec.l10n.localize("Mix");
            const $open = $h.fieldOpen("mix", $title, false, "", false);
            sri.pushContext();
            try {
                $h.textLine({"name":"text-line","attrs":{},"parent":{"name":"conditional-field","attrs":{"condition":"mode == 'a'"},"parent":{"name":"field","attrs":{"name":"mix"},"parent":{"name":"form-single","attrs":{"name":"Everything","map":"fv"}}}}});
            } finally {
                sri.popContext();
            }
            $h.fieldClose($open, false);
        }
    } else {
        {
            const $title = ec.l10n.localize("Mix");
            const $open = $h.fieldOpen("mix", $title, false, "", false);
            sri.pushContext();
            try {
                $h.display({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"mix"},"parent":{"name":"form-single","attrs":{"name":"Everything","map":"fv"}}}}});
            } finally {
                sri.popContext();
            }
            $h.fieldClose($open, false);
        }
    }
    {
        const $title = ec.l10n.localize("Go Now");
        const $open = $h.fieldOpen("go", $title, true, "", false);
        sri.pushContext();
        try {
            $h.submit({"name":"submit","attrs":{"confirmation":"Sure?","icon":"fa fa-check"},"parent":{"name":"default-field","attrs":{"title":"Go Now"},"parent":{"name":"field","attrs":{"name":"go"},"parent":{"name":"form-single","attrs":{"name":"Everything","map":"fv"}}}}}, ec.l10n.localize("Go Now"));
        } finally {
            sri.popContext();
        }
        $h.fieldClose($open, false);
    }
    {
        const $title = ec.l10n.localize("Via Link");
        const $open = $h.fieldOpen("viaLink", $title, false, "", false);
        sri.pushContext();
        try {
            $h.link({"name":"link","attrs":{"url":"open","text":"Open","link-type":"auto"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"viaLink"},"parent":{"name":"form-single","attrs":{"name":"Everything","map":"fv"}}}}}, true);
        } finally {
            sri.popContext();
        }
        $h.fieldClose($open, false);
    }
    $h.formClose({"name":"form-single","attrs":{"name":"Everything","map":"fv","background-submit":"true","background-reload-id":"Dyn","focus-field":"firstName","transition":"save","pass-through-parameters":"true"}});
} finally {
    sri.popContext();
}
if (sri.doBoundaryComments()) sri.write("<!-- END   form-single[@name=Everything] -->\n");
if (sri.doBoundaryComments()) sri.write("<!-- END   screen[@location=" + sri.getActiveScreenDef().location + "].widgets -->\n");

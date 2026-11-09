var total = price * 2;
var label = "Total: " + total;
$o.push(`${$s(label)} (${$s((total > 10) ? "big" : "small")})\nEscaped \${literal} and <#if not a tag>\n`);

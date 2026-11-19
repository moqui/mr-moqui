$o.push("Start\n");
{
let $e1 = false;
if (status == "open") {
$e1 = true;
$o.push(`Open ${$s(count)}\n`);
}
if (!$e1 && (status == "closed")) {
$e1 = true;
$o.push("Closed\n");
}
if (!$e1) {
$o.push("Other\n");
}
}
if (!items.length) {
$o.push("No items");
} else {
$o.push(`${$s(items.length)} items`);
}
$o.push("\nEnd\n");

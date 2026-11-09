$o.push("Start\n");
if (status == "open") {
$o.push(`Open ${$s(count)}\n`);
} else if (status == "closed") {
$o.push("Closed\n");
} else {
$o.push("Other\n");
}
if (!items.length) {
$o.push("No items");
} else {
$o.push(`${$s(items.length)} items`);
}
$o.push("\nEnd\n");

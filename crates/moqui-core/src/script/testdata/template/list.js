{
const $l1 = $list(rows);
for (let row_index = 0; row_index < $l1.length; row_index++) {
const row = $l1[row_index];
const row_has_next = row_index < $l1.length - 1;
$o.push(`${$s(row_index)}: ${$s(row.name)}`);
if (row_has_next) {
$o.push(",");
}
$o.push("\n");
{
const $l2 = $list(row.tags);
for (let tag_index = 0; tag_index < $l2.length; tag_index++) {
const tag = $l2[tag_index];
const tag_has_next = tag_index < $l2.length - 1;
$o.push(`  - ${$s(tag)}\n`);
}
}
}
}
{
const $l3 = $list(missing);
for (let nothing_index = 0; nothing_index < $l3.length; nothing_index++) {
const nothing = $l3[nothing_index];
const nothing_has_next = nothing_index < $l3.length - 1;
$o.push("never");
}
}
$o.push("done\n");

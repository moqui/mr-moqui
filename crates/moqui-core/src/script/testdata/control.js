(function() {
    if (ready) {
        statusId = orderStatus;
    } else {
        statusId = ec.resource.expand("N");
    }
    while (ready) {
        n = n + 1;
    }
    if (rows != null) {
        var _row_list = rows;
        var row_index = 0;
        var _row_cursor = typeof _row_list.length !== "number" && typeof _row_list.hasNext === "function" && typeof _row_list.next === "function";
        var _row_ahead = _row_cursor ? _row_list.next() : null;
        var row_len = _row_cursor ? Infinity : (typeof _row_list.length === "number") ? _row_list.length : 0;
        for (; row_index < row_len; row_index = row_index + 1) {
            if (_row_cursor) {
                if (_row_ahead == null) break;
                row = _row_ahead;
                _row_ahead = _row_list.next();
                row_has_next = _row_ahead != null;
            } else {
                row = (typeof _row_list.get === "function") ? _row_list.get(row_index) : _row_list[row_index];
                row_has_next = (row_index + 1) < row_len;
            }
            n = row;
        }
    }
    return;
})()

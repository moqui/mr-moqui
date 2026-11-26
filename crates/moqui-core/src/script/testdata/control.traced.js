(function() {
    __moquiTrace(0, "0");
    if (ready) {
        __moquiTrace(0, "0.0.0");
        statusId = orderStatus;
        __moquiTrace(1, "0.0.0", ["statusId"], [(() => { try { return statusId; } catch (e) { return undefined; } })()]);
    } else {
        __moquiTrace(0, "0.1.0");
        statusId = "N";
        __moquiTrace(1, "0.1.0", ["statusId"], [(() => { try { return statusId; } catch (e) { return undefined; } })()]);
    }
    __moquiTrace(1, "0");
    __moquiTrace(0, "1");
    while (ready) {
        __moquiTrace(0, "1.0");
        n = n + 1;
        __moquiTrace(1, "1.0", ["n"], [(() => { try { return n; } catch (e) { return undefined; } })()]);
    }
    __moquiTrace(1, "1");
    __moquiTrace(0, "2");
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
            __moquiTrace(0, "2.0");
            n = row;
            __moquiTrace(1, "2.0", ["n"], [(() => { try { return n; } catch (e) { return undefined; } })()]);
        }
    }
    __moquiTrace(1, "2", ["rows","row"], [(() => { try { return rows; } catch (e) { return undefined; } })(), (() => { try { return row; } catch (e) { return undefined; } })()]);
    return;
})()

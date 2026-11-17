/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

// A .qvue compiled for the runtime build of Vue (moqui_core::screen::vue_check::qvue_make), its templates as
// render functions; moqui.bundledComponent(name) makes it when it is first used.
(function (moqui) {
    moqui.qvtBundled = moqui.qvtBundled || {};
    moqui.qvtBundled["/Island"] = { make: function (Vue) {
        const module = { exports: {} };
        const define = moqui.define;
        // the file's script as it is (not indented: a template string in it keeps its text)
        const returned = (function (exports) {
module.exports = {
    name: 'IslandFixture',
    components: {
        'island-note': { props: ['text'], template: '<small class="island-note">{{ text }}</small>' },
    },
    data() {
        return { title: 'Bundled island', items: ['one', 'two'], count: 0 }
    },
}
        })(module.exports);
        const component = Object.assign({}, returned !== undefined ? returned : module.exports);
        {
            const target = component;
            target.render = (function () {
                const { toDisplayString: _toDisplayString, createElementVNode: _createElementVNode, renderList: _renderList, Fragment: _Fragment, openBlock: _openBlock, createElementBlock: _createElementBlock, resolveComponent: _resolveComponent, createVNode: _createVNode } = Vue

                return function render(_ctx, _cache) {
                  const _component_island_note = _resolveComponent("island-note")

                  return (_openBlock(), _createElementBlock("div", { class: "island-fixture" }, [
                    _createElementVNode("p", { class: "island-title" }, _toDisplayString(_ctx.title), 1),
                    _createElementVNode("ul", null, [
                      (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.items, (item) => {
                        return (_openBlock(), _createElementBlock("li", { key: item }, _toDisplayString(item), 1))
                      }), 128))
                    ]),
                    _createElementVNode("button", {
                      type: "button",
                      class: "island-add",
                      onClick: $event => (_ctx.count++)
                    }, "Added " + _toDisplayString(_ctx.count), 9, ["onClick"]),
                    _createVNode(_component_island_note, {
                      text: _ctx.$root.context.note
                    }, null, 8, ["text"])
                  ]))
                }
            })();
            delete target.template;
        }
        {
            const target = component.components["island-note"];
            target.render = (function () {
                const { toDisplayString: _toDisplayString, openBlock: _openBlock, createElementBlock: _createElementBlock } = Vue

                return function render(_ctx, _cache) {
                  return (_openBlock(), _createElementBlock("small", { class: "island-note" }, _toDisplayString(_ctx.text), 1))
                }
            })();
            delete target.template;
        }
        return { component: component, style: "\n.island-fixture .island-note { font-style: italic; }\n", scoped: false };
    } };
})(globalThis.moqui);

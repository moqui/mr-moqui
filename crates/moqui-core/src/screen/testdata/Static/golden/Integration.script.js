(function(){var m=globalThis.moqui=globalThis.moqui||{};(m.qvtStatic=m.qvtStatic||{})["/Integration"]=function(Vue){
const { toDisplayString: _toDisplayString, createElementVNode: _createElementVNode, resolveComponent: _resolveComponent, withCtx: _withCtx, createVNode: _createVNode, Fragment: _Fragment, openBlock: _openBlock, createElementBlock: _createElementBlock, createCommentVNode: _createCommentVNode, normalizeClass: _normalizeClass } = Vue

return function render(_ctx, _cache) {
  const _component_m_link = _resolveComponent("m-link")
  const _component_m_display = _resolveComponent("m-display")
  const _component_m_drop_down = _resolveComponent("m-drop-down")
  const _component_m_form = _resolveComponent("m-form")
  const _component_m_scope = _resolveComponent("m-scope")
  const _component_m_static_form_list = _resolveComponent("m-static-form-list")

  return (_openBlock(), _createElementBlock(_Fragment, null, [
    _createVNode(_component_m_link, {
      href: _ctx.$mq.url('/Integration',{},_ctx.$mq.merge({'overridden':_ctx.ec.l10n.localize('response'),'target':'source','more':'inline'},null)),
      disable: _ctx.$scope['mqDisable_db888939440d6259']!==false,
      class: "m-link-text"
    }, {
      default: _withCtx(() => [
        _createElementVNode("span", {
          textContent: _toDisplayString(_ctx.ec.l10n.localize('Alias'))
        }, null, 8, ["textContent"])
      ]),
      _: 1
    }, 8, ["href", "disable"]),
    _createElementVNode("a", {
      href: (_ctx.$scope['mqDisable_b0d197f89f81b8a6']!==false)?undefined:(_ctx.$mq.url('/Integration/go',{'kind':_ctx.ec.l10n.localize('own')},_ctx.$mq.merge({'kind':'inline','x':'one two'},null))),
      disable: _ctx.$scope['mqDisable_b0d197f89f81b8a6']!==false,
      class: "m-link-text",
      "aria-disabled": _ctx.$scope['mqDisable_b0d197f89f81b8a6']!==false,
      tabindex: (_ctx.$scope['mqDisable_b0d197f89f81b8a6']!==false)?-1:undefined,
      onClick: $event => ((_ctx.$scope['mqDisable_b0d197f89f81b8a6']!==false) && $event.preventDefault())
    }, [
      _createElementVNode("span", {
        textContent: _toDisplayString(_ctx.ec.l10n.localize('Inline'))
      }, null, 8, ["textContent"])
    ], 8, ["href", "disable", "aria-disabled", "tabindex", "onClick"]),
    _createElementVNode("img", {
      src: _ctx.$mq.url('/Integration/go',{},_ctx.$mq.merge({'image':'yes'},null)),
      alt: _ctx.ec.l10n.localize('Inline image')
    }, null, 8, ["src", "alt"]),
    _createVNode(_component_m_scope, {
      value: (_ctx.fieldValues),
      extra: _ctx.$scope['mqExtra_Edit']
    }, {
      default: _withCtx((_ctx) => [
        _createElementVNode("div", { class: "q-my-md" }, [
          _createVNode(_component_m_form, {
            name: "Edit",
            id: "Edit",
            action: "/Integration/go",
            disable: _ctx.$scope['mqDisable_88b05b2462ae180']!==false,
            "read-only": "",
            "fields-initial": {'day':_ctx.$mq.dflt(_ctx.$scope['mqPlain_9862720f1938adc5'],_ctx.$mq.str(_ctx.$scope['day'])),'clock':_ctx.$mq.dflt(_ctx.$scope['mqPlain_6e3bc21af98e07d7'],_ctx.$mq.str(_ctx.$scope['clock'])),'clock_display':_ctx.$mq.str(_ctx.$scope['clock']),'dateText':_ctx.$mq.dflt(_ctx.$scope['mqPlain_6296d76cdefd7296'],_ctx.$mq.str(_ctx.$scope['dateText'])),'lookup':_ctx.$mq.cur(_ctx.$scope['lookup'],false,''),'moquiFormName':'Edit'}
          }, {
            default: _withCtx((formProps) => [
              _createElementVNode("div", { class: "q-ma-sm" }, [
                _createVNode(_component_m_display, {
                  name: "clock",
                  "label-wrapper": "",
                  id: "Edit_clock_display",
                  label: _ctx.ec.l10n.localize('Clock'),
                  modelValue: formProps.fields.clock,
                  "onUpdate:modelValue": $event => ((formProps.fields.clock) = $event),
                  display: formProps.fields.clock_display,
                  fields: formProps.fields
                }, null, 8, ["label", "modelValue", "onUpdate:modelValue", "display", "fields"])
              ]),
              _createElementVNode("div", { class: "q-ma-sm" }, [
                _createVNode(_component_m_drop_down, {
                  name: "lookup",
                  class: "dynamic-options",
                  id: "Edit_lookup",
                  label: _ctx.ec.l10n.localize('Lookup'),
                  options: (_ctx.$scope['mqOpt_Edit_lookup_0_0']||[]),
                  "options-url": "/Integration/go",
                  "value-field": "value",
                  "label-field": "label",
                  "depends-on": {},
                  "options-parameters": _ctx.$mq.merge(_ctx.$mq.given({'lookup':'yes','scope':'all'}),null,{}),
                  modelValue: formProps.fields.lookup,
                  "onUpdate:modelValue": $event => ((formProps.fields.lookup) = $event)
                }, null, 8, ["label", "options", "options-parameters", "modelValue", "onUpdate:modelValue"])
              ])
            ]),
            _: 1
          }, 8, ["disable", "fields-initial"])
        ])
      ]),
      _: 1
    }, 8, ["value", "extra"]),
    _createVNode(_component_m_static_form_list, {
      name: "Rows",
      id: "Rows",
      action: "/Integration/go",
      disable: _ctx.$scope['mqDisable_534ed201d6b5c9ca']!==false,
      "disable-key": "mqDisable_534ed201d6b5c9ca",
      "list-name": "rows"
    }, {
      header: _withCtx((hs) => [
        _createElementVNode("div", { class: "th text-left" }, [
          _createElementVNode("div", null, [
            _createElementVNode("div", { class: "form-title" }, [
              _createElementVNode("span", {
                textContent: _toDisplayString(_ctx.ec.l10n.localize('ID'))
              }, null, 8, ["textContent"])
            ])
          ])
        ]),
        _createElementVNode("div", { class: "th text-left" }, [
          _createElementVNode("div", null, [
            _createElementVNode("div", { class: "form-title" }, [
              _createElementVNode("span", {
                textContent: _toDisplayString(_ctx.ec.l10n.localize('Day'))
              }, null, 8, ["textContent"])
            ])
          ])
        ])
      ]),
      row: _withCtx((_ctx) => [
        _createElementVNode("div", {
          class: _normalizeClass(["tr", {'text-weight-bold': _ctx._moquiRowType === 'total'}])
        }, [
          _createVNode(_component_m_form, {
            name: 'Rows_'+_ctx.rowIndex,
            id: 'Rows_'+_ctx.rowIndex,
            action: "/Integration/go",
            disable: _ctx.$scope['mqDisable_534ed201d6b5c9ca']!==false,
            "fields-initial": {'id':_ctx.$mq.dflt(_ctx.$scope['mqPlain_4773273dff14d0c'],_ctx.$mq.str(_ctx.$scope['id'])),'id_display':_ctx.$mq.str(_ctx.$scope['id']),'day':((_ctx.id === 'first')?_ctx.$mq.dflt(_ctx.$scope['mqPlain_e5514715cc65b7d9'],_ctx.$mq.str(_ctx.$scope['day'])):_ctx.$mq.dflt(_ctx.$scope['mqPlain_20ecdbf086bf711'],_ctx.$mq.str(_ctx.$scope['day']))),'day_display':_ctx.$mq.str(_ctx.$scope['day']),'clock':_ctx.$mq.dflt(_ctx.$scope['mqPlain_3b04c76509151957'],_ctx.$mq.str(_ctx.$scope['clock'])),'moquiFormName':'Rows','pageIndex':_ctx.$mq.str(_ctx.$scope['pageIndex']||0)}
          }, {
            default: _withCtx((formProps) => [
              _createElementVNode("div", { class: "td" }, [
                _createElementVNode("div", { class: "form-group" }, [
                  _createVNode(_component_m_display, {
                    name: "id",
                    id: 'Rows_id'+'_'+_ctx.rowIndex+'_display',
                    modelValue: formProps.fields.id,
                    "onUpdate:modelValue": $event => ((formProps.fields.id) = $event),
                    display: formProps.fields.id_display,
                    fields: formProps.fields
                  }, null, 8, ["id", "modelValue", "onUpdate:modelValue", "display", "fields"])
                ])
              ]),
              _createElementVNode("div", { class: "td" }, [
                ((_ctx.id === 'first'))
                  ? (_openBlock(), _createElementBlock(_Fragment, { key: 0 }, [], 64))
                  : (_openBlock(), _createElementBlock("div", {
                      key: 1,
                      class: "form-group"
                    }, [
                      _createVNode(_component_m_display, {
                        name: "day",
                        id: 'Rows_day'+'_'+_ctx.rowIndex+'_display',
                        modelValue: formProps.fields.day,
                        "onUpdate:modelValue": $event => ((formProps.fields.day) = $event),
                        display: formProps.fields.day_display,
                        fields: formProps.fields
                      }, null, 8, ["id", "modelValue", "onUpdate:modelValue", "display", "fields"])
                    ]))
              ])
            ]),
            _: 1
          }, 8, ["name", "id", "disable", "fields-initial"])
        ], 2)
      ]),
      _: 1
    }, 8, ["disable"])
  ], 64))
}
};})();

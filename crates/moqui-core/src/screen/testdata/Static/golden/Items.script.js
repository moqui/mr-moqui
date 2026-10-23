(function(){var m=globalThis.moqui=globalThis.moqui||{};(m.qvtStatic=m.qvtStatic||{})["/Items"]=function(Vue){
const { toDisplayString: _toDisplayString, createElementVNode: _createElementVNode, resolveComponent: _resolveComponent, createVNode: _createVNode, openBlock: _openBlock, createElementBlock: _createElementBlock, createCommentVNode: _createCommentVNode, withCtx: _withCtx, createBlock: _createBlock, renderList: _renderList, Fragment: _Fragment, resolveDirective: _resolveDirective, withDirectives: _withDirectives, withModifiers: _withModifiers, normalizeClass: _normalizeClass } = Vue

return function render(_ctx, _cache) {
  const _component_m_display = _resolveComponent("m-display")
  const _component_m_text_line = _resolveComponent("m-text-line")
  const _component_m_drop_down = _resolveComponent("m-drop-down")
  const _component_q_btn = _resolveComponent("q-btn")
  const _component_m_form = _resolveComponent("m-form")
  const _component_m_scope = _resolveComponent("m-scope")
  const _component_m_form_column_config = _resolveComponent("m-form-column-config")
  const _component_m_container_dialog = _resolveComponent("m-container-dialog")
  const _component_q_item_section = _resolveComponent("q-item-section")
  const _component_q_item = _resolveComponent("q-item")
  const _component_q_list = _resolveComponent("q-list")
  const _component_q_btn_dropdown = _resolveComponent("q-btn-dropdown")
  const _component_q_icon = _resolveComponent("q-icon")
  const _component_m_static_form_list = _resolveComponent("m-static-form-list")
  const _directive_close_popup = _resolveDirective("close-popup")

  return (_openBlock(), _createElementBlock(_Fragment, null, [
    _createElementVNode("h4", {
      class: "text-inline",
      textContent: _toDisplayString(_ctx.ec.resource.expand('Hello ${greeting}',{'greeting':()=>(_ctx.greeting)}))
    }, null, 8, ["textContent"]),
    ((_ctx.item != null))
      ? (_openBlock(), _createBlock(_component_m_scope, {
          key: 0,
          value: (_ctx.item),
          extra: _ctx.$scope['mqExtra_ItemForm']
        }, {
          default: _withCtx((_ctx) => [
            _createElementVNode("div", { class: "q-my-md" }, [
              _createVNode(_component_m_form, {
                name: "ItemForm",
                id: "ItemForm",
                action: "/Items/saveItem",
                "read-only": "",
                "fields-initial": {'itemId':_ctx.$mq.str(_ctx.$scope['itemId']),'itemId_display':_ctx.$mq.str(_ctx.$scope['itemId']),'name':_ctx.$mq.str(_ctx.$scope['name']),'status':_ctx.$mq.cur(_ctx.$scope['status'],false,''),'price':_ctx.$mq.str(_ctx.$scope['price']),'price_display':_ctx.$mq.fmt(_ctx.$scope['price'],'#,##0.00'),'mode':((_ctx.status == 'ACTIVE')?_ctx.$mq.str(_ctx.$scope['mode']):_ctx.$mq.str(_ctx.$scope['mode'])),'mode_display':((_ctx.status == 'ACTIVE')?_ctx.ec.resource.expand('is active: ${name}',{'name':()=>(_ctx.name)}):_ctx.ec.l10n.localize('not active')),'moquiFormName':'ItemForm'}
              }, {
                default: _withCtx((formProps) => [
                  _createElementVNode("div", { class: "q-ma-sm" }, [
                    _createVNode(_component_m_display, {
                      name: "itemId",
                      "label-wrapper": "",
                      id: "ItemForm_itemId_display",
                      label: _ctx.ec.l10n.localize('Item ID'),
                      modelValue: formProps.fields.itemId,
                      "onUpdate:modelValue": $event => ((formProps.fields.itemId) = $event),
                      display: formProps.fields.itemId_display,
                      fields: formProps.fields
                    }, null, 8, ["label", "modelValue", "onUpdate:modelValue", "display", "fields"])
                  ]),
                  _createElementVNode("div", { class: "q-ma-sm" }, [
                    _createVNode(_component_m_text_line, {
                      id: "ItemForm_name",
                      type: "text",
                      name: "name",
                      size: "30",
                      label: _ctx.ec.l10n.localize('Item Name'),
                      modelValue: formProps.fields.name,
                      "onUpdate:modelValue": $event => ((formProps.fields.name) = $event),
                      fields: formProps.fields
                    }, null, 8, ["label", "modelValue", "onUpdate:modelValue", "fields"])
                  ]),
                  _createElementVNode("div", { class: "q-ma-sm" }, [
                    _createVNode(_component_m_drop_down, {
                      name: "status",
                      id: "ItemForm_status",
                      label: _ctx.ec.l10n.localize('Status'),
                      options: [{value:'NONE',label:_ctx.ec.l10n.localize('None')},{value:'ACTIVE',label:_ctx.ec.l10n.localize('Active')}],
                      modelValue: formProps.fields.status,
                      "onUpdate:modelValue": $event => ((formProps.fields.status) = $event)
                    }, null, 8, ["label", "options", "modelValue", "onUpdate:modelValue"])
                  ]),
                  _createElementVNode("div", { class: "q-ma-sm" }, [
                    _createVNode(_component_m_display, {
                      name: "price",
                      "label-wrapper": "",
                      id: "ItemForm_price_display",
                      label: _ctx.ec.l10n.localize('Price'),
                      modelValue: formProps.fields.price,
                      "onUpdate:modelValue": $event => ((formProps.fields.price) = $event),
                      display: formProps.fields.price_display,
                      fields: formProps.fields
                    }, null, 8, ["label", "modelValue", "onUpdate:modelValue", "display", "fields"])
                  ]),
                  ((_ctx.status == 'ACTIVE'))
                    ? (_openBlock(), _createElementBlock("div", {
                        key: 0,
                        class: "q-ma-sm"
                      }, [
                        _createVNode(_component_m_display, {
                          name: "mode",
                          "label-wrapper": "",
                          id: "ItemForm_mode_display",
                          label: _ctx.ec.l10n.localize('Mode'),
                          modelValue: formProps.fields.mode,
                          "onUpdate:modelValue": $event => ((formProps.fields.mode) = $event),
                          display: formProps.fields.mode_display,
                          fields: formProps.fields
                        }, null, 8, ["label", "modelValue", "onUpdate:modelValue", "display", "fields"])
                      ]))
                    : (_openBlock(), _createElementBlock("div", {
                        key: 1,
                        class: "q-ma-sm"
                      }, [
                        _createVNode(_component_m_display, {
                          name: "mode",
                          "label-wrapper": "",
                          id: "ItemForm_mode_display",
                          label: _ctx.ec.l10n.localize('Mode'),
                          modelValue: formProps.fields.mode,
                          "onUpdate:modelValue": $event => ((formProps.fields.mode) = $event),
                          display: formProps.fields.mode_display,
                          fields: formProps.fields
                        }, null, 8, ["label", "modelValue", "onUpdate:modelValue", "display", "fields"])
                      ])),
                  _createElementVNode("div", { class: "q-ma-sm" }, [
                    _createVNode(_component_q_btn, {
                      dense: "",
                      outline: !formProps.hasFieldsChanged,
                      unelevated: formProps.hasFieldsChanged,
                      "no-caps": "",
                      type: "submit",
                      name: "go",
                      value: "go",
                      color: "primary",
                      id: "ItemForm_go"
                    }, {
                      default: _withCtx(() => [
                        _createElementVNode("span", {
                          textContent: _toDisplayString(_ctx.ec.l10n.localize('Save'))
                        }, null, 8, ["textContent"])
                      ]),
                      _: 1
                    }, 8, ["outline", "unelevated"])
                  ])
                ]),
                _: 1
              }, 8, ["fields-initial"])
            ])
          ]),
          _: 1
        }, 8, ["value", "extra"]))
      : (_openBlock(), _createElementBlock("span", {
          key: 1,
          class: "text-inline",
          textContent: _toDisplayString(_ctx.ec.l10n.localize('No item'))
        }, null, 8, ["textContent"])),
    (_openBlock(true), _createElementBlock(_Fragment, null, _renderList((_ctx.tags), (tag, tag_index) => {
      return (_openBlock(), _createElementBlock("div", {
        key: tag_index,
        class: "text-inline",
        textContent: _toDisplayString(_ctx.ec.resource.expand('Tag ${tag} of ${item.name}',{'tag':()=>(tag),'item.name':()=>(_ctx.item.name)}))
      }, null, 8, ["textContent"]))
    }), 128)),
    _createElementVNode("a", {
      href: _ctx.$mq.url('/Items/viewItem',{'itemId':(_ctx.item.itemId)},_ctx.$mq.merge({'pageIndex':_ctx.$scope['pageIndex']},null)),
      class: "m-link-text"
    }, [
      _createElementVNode("span", {
        textContent: _toDisplayString(_ctx.ec.resource.expand('View ${item.itemId}',{'item.itemId':()=>(_ctx.item.itemId)}))
      }, null, 8, ["textContent"])
    ], 8, ["href"]),
    _createVNode(_component_m_static_form_list, {
      name: "ItemList",
      id: "ItemList",
      action: "/Items/saveItem",
      "list-name": "itemList",
      "row-initial": (_ctx) => ({'itemId':_ctx.$mq.str(_ctx.$scope['itemId']),'itemId_display':_ctx.$mq.str(_ctx.$scope['itemId']),'name':_ctx.$mq.str(_ctx.$scope['name']),'price':_ctx.$mq.str(_ctx.$scope['price']),'price_display':_ctx.$mq.fmt(_ctx.$scope['price'],'#,##0.00')}),
      "default-columns": [['itemId'],['name'],['price'],['go']],
      "column-fields": [{id:'itemId',label:_ctx.ec.l10n.localize('Item ID')},{id:'name',label:_ctx.ec.l10n.localize('Name')},{id:'price',label:_ctx.ec.l10n.localize('Price')},{id:'go',label:_ctx.ec.l10n.localize('Save')}],
      "columns-action": "/Items/formSelectColumns",
      "form-location": "ItemList"
    }, {
      nav: _withCtx((nv) => [
        (_ctx.$scope['hideNav']!=='true')
          ? (_openBlock(), _createElementBlock(_Fragment, { key: 0 }, [
              _createVNode(_component_m_container_dialog, {
                id: "ItemList_SelColsDialog",
                title: _ctx.ec.l10n.localize('Column Fields')
              }, {
                button: _withCtx(() => [
                  _createVNode(_component_q_btn, {
                    dense: "",
                    unelevated: "",
                    "no-caps": "",
                    class: "m-btn-tonal",
                    icon: "view_column",
                    label: _ctx.ec.l10n.localize('Columns')
                  }, null, 8, ["label"])
                ]),
                default: _withCtx(() => [
                  _createVNode(_component_m_form_column_config, {
                    id: "ItemList_SelColsForm",
                    action: nv.columnsAction,
                    "find-parameters": nv.params,
                    "columns-initial": nv.columnsTree,
                    "form-location": "ItemList"
                  }, null, 8, ["action", "find-parameters", "columns-initial"])
                ]),
                _: 2
              }, 1032, ["title"]),
              _createVNode(_component_q_btn_dropdown, {
                dense: "",
                flat: "",
                "no-caps": "",
                icon: "download",
                label: _ctx.ec.l10n.localize('Export')
              }, {
                default: _withCtx(() => [
                  _createVNode(_component_q_list, { dense: "" }, {
                    default: _withCtx(() => [
                      _withDirectives((_openBlock(), _createBlock(_component_q_item, {
                        clickable: "",
                        tag: "a",
                        href: nv.exportUrl('csv')
                      }, {
                        default: _withCtx(() => [
                          _createVNode(_component_q_item_section, null, {
                            default: _withCtx(() => [
                              _createElementVNode("span", {
                                textContent: _toDisplayString(_ctx.ec.l10n.localize('CSV'))
                              }, null, 8, ["textContent"])
                            ]),
                            _: 1
                          })
                        ]),
                        _: 1
                      }, 8, ["href"])), [
                        [_directive_close_popup]
                      ])
                    ]),
                    _: 2
                  }, 1024)
                ]),
                _: 2
              }, 1032, ["label"])
            ], 64))
          : _createCommentVNode("", true)
      ]),
      hc_itemId: _withCtx(({hs, formProps}) => [
        _createElementVNode("div", null, [
          _createElementVNode("div", { class: "form-title" }, [
            _createElementVNode("span", {
              textContent: _toDisplayString(_ctx.ec.l10n.localize('Item ID'))
            }, null, 8, ["textContent"]),
            _createElementVNode("span", { class: "form-order-by" }, [
              _createElementVNode("a", {
                href: "#",
                onClick: _withModifiers($event => (hs.sortBy('itemId')), ["prevent"]),
                class: _normalizeClass({active: hs.orderBy('itemId') === 'asc'})
              }, [
                _createVNode(_component_q_icon, { name: "arrow_drop_up" })
              ], 10, ["onClick"]),
              _createElementVNode("a", {
                href: "#",
                onClick: _withModifiers($event => (hs.sortBy('-itemId')), ["prevent"]),
                class: _normalizeClass({active: hs.orderBy('-itemId') === 'desc'})
              }, [
                _createVNode(_component_q_icon, { name: "arrow_drop_down" })
              ], 10, ["onClick"])
            ])
          ])
        ])
      ]),
      hc_name: _withCtx(({hs, formProps}) => [
        _createElementVNode("div", null, [
          _createElementVNode("div", { class: "form-title" }, [
            _createElementVNode("span", {
              textContent: _toDisplayString(_ctx.ec.l10n.localize('Name'))
            }, null, 8, ["textContent"])
          ])
        ])
      ]),
      hc_price: _withCtx(({hs, formProps}) => [
        _createElementVNode("div", null, [
          _createElementVNode("div", { class: "form-title text-right" }, [
            _createElementVNode("span", {
              textContent: _toDisplayString(_ctx.ec.l10n.localize('Price'))
            }, null, 8, ["textContent"])
          ])
        ])
      ]),
      hc_go: _withCtx(({hs, formProps}) => [
        _createElementVNode("div", null, [
          _createElementVNode("div", { class: "form-title" })
        ])
      ]),
      rc_itemId: _withCtx((_ctx) => [
        _createElementVNode("div", { class: "form-group" }, [
          _createVNode(_component_m_display, {
            name: "itemId",
            id: 'ItemList_itemId'+'_'+_ctx.rowIndex+'_display',
            modelValue: _ctx.formProps.fields.itemId,
            "onUpdate:modelValue": $event => ((_ctx.formProps.fields.itemId) = $event),
            display: _ctx.formProps.fields.itemId_display,
            fields: _ctx.formProps.fields
          }, null, 8, ["id", "modelValue", "onUpdate:modelValue", "display", "fields"])
        ])
      ]),
      rc_name: _withCtx((_ctx) => [
        _createElementVNode("div", { class: "form-group" }, [
          _createVNode(_component_m_text_line, {
            id: 'ItemList_name'+'_'+_ctx.rowIndex,
            type: "text",
            name: "name",
            size: "30",
            label: _ctx.ec.l10n.localize('Name'),
            modelValue: _ctx.formProps.fields.name,
            "onUpdate:modelValue": $event => ((_ctx.formProps.fields.name) = $event),
            fields: _ctx.formProps.fields
          }, null, 8, ["id", "label", "modelValue", "onUpdate:modelValue", "fields"])
        ])
      ]),
      rc_price: _withCtx((_ctx) => [
        _createElementVNode("div", { class: "form-group text-right" }, [
          _createVNode(_component_m_display, {
            name: "price",
            class: "text-right",
            id: 'ItemList_price'+'_'+_ctx.rowIndex+'_display',
            modelValue: _ctx.formProps.fields.price,
            "onUpdate:modelValue": $event => ((_ctx.formProps.fields.price) = $event),
            display: _ctx.formProps.fields.price_display,
            fields: _ctx.formProps.fields
          }, null, 8, ["id", "modelValue", "onUpdate:modelValue", "display", "fields"])
        ])
      ]),
      rc_go: _withCtx((_ctx) => [
        _createElementVNode("div", { class: "form-group" }, [
          _createVNode(_component_q_btn, {
            dense: "",
            outline: !_ctx.formProps.hasFieldsChanged,
            unelevated: _ctx.formProps.hasFieldsChanged,
            "no-caps": "",
            type: "submit",
            name: "go",
            value: "go",
            color: "primary",
            id: 'ItemList_go'+'_'+_ctx.rowIndex
          }, {
            default: _withCtx(() => [
              _createElementVNode("span", {
                textContent: _toDisplayString(_ctx.ec.l10n.localize('Save'))
              }, null, 8, ["textContent"])
            ]),
            _: 1
          }, 8, ["outline", "unelevated", "id"])
        ])
      ]),
      _: 1
    }, 8, ["row-initial", "column-fields"]),
    _createVNode(_component_m_static_form_list, {
      name: "ItemMulti",
      id: "ItemMulti",
      action: "/Items/saveItem",
      "list-name": "itemList",
      multi: "",
      "row-initial": (_ctx) => ({'itemId':_ctx.$mq.str(_ctx.$scope['itemId']),'itemId_display':_ctx.$mq.str(_ctx.$scope['itemId']),'name':_ctx.$mq.str(_ctx.$scope['name'])})
    }, {
      header: _withCtx((hs) => [
        _createElementVNode("div", { class: "th text-left" }, [
          _createElementVNode("div", null, [
            _createElementVNode("div", { class: "form-title" }, [
              _createElementVNode("span", {
                textContent: _toDisplayString(_ctx.ec.l10n.localize('Item ID'))
              }, null, 8, ["textContent"])
            ])
          ])
        ]),
        _createElementVNode("div", { class: "th text-left" }, [
          _createElementVNode("div", null, [
            _createElementVNode("div", { class: "form-title" }, [
              _createElementVNode("span", {
                textContent: _toDisplayString(_ctx.ec.l10n.localize('Name'))
              }, null, 8, ["textContent"])
            ])
          ])
        ]),
        _createElementVNode("div", { class: "th text-left" }, [
          _createElementVNode("div", null, [
            _createElementVNode("div", { class: "form-title" })
          ])
        ])
      ]),
      row: _withCtx((_ctx) => [
        _createElementVNode("div", {
          class: _normalizeClass(["tr", {'text-weight-bold': _ctx._moquiRowType === 'total'}])
        }, [
          _createElementVNode("div", { class: "td" }, [
            _createElementVNode("div", { class: "form-group" }, [
              _createVNode(_component_m_display, {
                name: "itemId",
                id: 'ItemMulti_itemId'+'_'+_ctx.rowIndex+'_display',
                modelValue: _ctx.formProps.fields.itemId,
                "onUpdate:modelValue": $event => ((_ctx.formProps.fields.itemId) = $event),
                display: _ctx.formProps.fields.itemId_display,
                fields: _ctx.formProps.fields
              }, null, 8, ["id", "modelValue", "onUpdate:modelValue", "display", "fields"])
            ])
          ]),
          _createElementVNode("div", { class: "td" }, [
            _createElementVNode("div", { class: "form-group" }, [
              _createVNode(_component_m_text_line, {
                id: 'ItemMulti_name'+'_'+_ctx.rowIndex,
                type: "text",
                name: "name",
                size: "30",
                label: _ctx.ec.l10n.localize('Name'),
                modelValue: _ctx.formProps.fields.name,
                "onUpdate:modelValue": $event => ((_ctx.formProps.fields.name) = $event),
                fields: _ctx.formProps.fields
              }, null, 8, ["id", "label", "modelValue", "onUpdate:modelValue", "fields"])
            ])
          ]),
          _createElementVNode("div", { class: "td" }, [
            _createElementVNode("div", { class: "form-group" }, [
              _createVNode(_component_q_btn, {
                dense: "",
                outline: !_ctx.formProps.hasFieldsChanged,
                unelevated: _ctx.formProps.hasFieldsChanged,
                "no-caps": "",
                type: "submit",
                name: "go",
                value: "go",
                color: "primary",
                id: 'ItemMulti_go'+'_'+_ctx.rowIndex
              }, {
                default: _withCtx(() => [
                  _createElementVNode("span", {
                    textContent: _toDisplayString(_ctx.ec.l10n.localize('Save All'))
                  }, null, 8, ["textContent"])
                ]),
                _: 1
              }, 8, ["outline", "unelevated", "id"])
            ])
          ])
        ], 2)
      ]),
      _: 1
    }, 8, ["row-initial"])
  ], 64))
}
};})();

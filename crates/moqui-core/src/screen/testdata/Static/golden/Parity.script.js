(function(){var m=globalThis.moqui=globalThis.moqui||{};(m.qvtStatic=m.qvtStatic||{})["/Parity"]=function(Vue){
const { toDisplayString: _toDisplayString, openBlock: _openBlock, createElementBlock: _createElementBlock, createCommentVNode: _createCommentVNode, createElementVNode: _createElementVNode, resolveComponent: _resolveComponent, withCtx: _withCtx, createVNode: _createVNode, createBlock: _createBlock, normalizeClass: _normalizeClass, renderList: _renderList, Fragment: _Fragment } = Vue

return function render(_ctx, _cache) {
  const _component_q_item_section = _resolveComponent("q-item-section")
  const _component_q_item = _resolveComponent("q-item")
  const _component_q_list = _resolveComponent("q-list")
  const _component_q_btn_dropdown = _resolveComponent("q-btn-dropdown")
  const _component_m_text_line = _resolveComponent("m-text-line")
  const _component_q_expansion_item = _resolveComponent("q-expansion-item")
  const _component_m_text_area = _resolveComponent("m-text-area")
  const _component_m_form = _resolveComponent("m-form")
  const _component_m_scope = _resolveComponent("m-scope")
  const _component_m_display = _resolveComponent("m-display")
  const _component_q_btn = _resolveComponent("q-btn")
  const _component_m_static_form_list = _resolveComponent("m-static-form-list")
  const _component_q_checkbox = _resolveComponent("q-checkbox")
  const _component_m_container_dialog = _resolveComponent("m-container-dialog")
  const _component_q_card_section = _resolveComponent("q-card-section")
  const _component_q_card = _resolveComponent("q-card")
  const _component_m_form_paginate = _resolveComponent("m-form-paginate")

  return (_openBlock(), _createElementBlock(_Fragment, null, [
    (((_ctx.$mq.compare(_ctx.status,'equals','ACTIVE',null,null,'Object') && (_ctx.qty > 3))))
      ? (_openBlock(), _createElementBlock("p", {
          key: 0,
          class: "text-inline",
          textContent: _toDisplayString(_ctx.ec.l10n.localize('the condition holds'))
        }, null, 8, ["textContent"]))
      : (_openBlock(), _createElementBlock("p", {
          key: 1,
          class: "text-inline",
          textContent: _toDisplayString(_ctx.ec.l10n.localize('the condition fails'))
        }, null, 8, ["textContent"])),
    (((_ctx.$mq.compare(_ctx.qty,'greater','10',null,null,'Long') || !((_ctx.status)))))
      ? (_openBlock(), _createElementBlock("p", {
          key: 2,
          class: "text-inline",
          textContent: _toDisplayString(_ctx.ec.l10n.localize('the second holds'))
        }, null, 8, ["textContent"]))
      : (_openBlock(), _createElementBlock("p", {
          key: 3,
          class: "text-inline",
          textContent: _toDisplayString(_ctx.ec.l10n.localize('the second fails'))
        }, null, 8, ["textContent"])),
    _createElementVNode("p", {
      class: "text-inline",
      textContent: _toDisplayString(_ctx.ec.resource.expand('Product ${productName} at ${price}',{'productName':()=>(_ctx.$mq.inMap((_ctx.product),'productName')?_ctx.$mq.inMap((_ctx.product),'productName',true):(_ctx.productName)),'price':()=>(_ctx.$mq.inMap((_ctx.product),'price')?_ctx.$mq.inMap((_ctx.product),'price',true):(_ctx.price))}))
    }, null, 8, ["textContent"]),
    _createElementVNode("a", {
      href: _ctx.$mq.url('/Parity/viewProduct',{},null),
      class: "m-link-text"
    }, [
      _createElementVNode("span", {
        textContent: _toDisplayString(_ctx.ec.resource.expand('View ${productName}',{'productName':()=>(_ctx.$mq.inMap((_ctx.product),'productName')?_ctx.$mq.inMap((_ctx.product),'productName',true):(_ctx.productName))}))
      }, null, 8, ["textContent"])
    ], 8, ["href"]),
    _createVNode(_component_q_btn_dropdown, {
      dense: "",
      unelevated: "",
      "no-caps": "",
      class: "m-btn-tonal text-primary"
    }, {
      label: _withCtx(() => [
        _createElementVNode("i", { class: "fa fa-bars q-icon on-left" }),
        _createElementVNode("span", {
          textContent: _toDisplayString(_ctx.ec.resource.expand('Menu of ${productName}',{'productName':()=>(_ctx.$mq.inMap((_ctx.product),'productName')?_ctx.$mq.inMap((_ctx.product),'productName',true):(_ctx.productName))}))
        }, null, 8, ["textContent"])
      ]),
      default: _withCtx(() => [
        _createVNode(_component_q_list, null, {
          default: _withCtx(() => [
            _createVNode(_component_q_item, { clickable: "" }, {
              default: _withCtx(() => [
                _createVNode(_component_q_item_section, null, {
                  default: _withCtx(() => [
                    _createElementVNode("a", {
                      href: _ctx.$mq.url('/Parity/viewProduct',{},null),
                      class: "m-link-text"
                    }, [
                      _createElementVNode("span", {
                        textContent: _toDisplayString(_ctx.ec.l10n.localize('View it'))
                      }, null, 8, ["textContent"])
                    ], 8, ["href"])
                  ]),
                  _: 1
                })
              ]),
              _: 1
            }),
            ((_ctx.qty > 3))
              ? (_openBlock(), _createBlock(_component_q_item, {
                  key: 0,
                  clickable: ""
                }, {
                  default: _withCtx(() => [
                    _createVNode(_component_q_item_section, null, {
                      default: _withCtx(() => [
                        ((_ctx.qty > 3))
                          ? (_openBlock(), _createElementBlock("span", {
                              key: 0,
                              class: "text-inline",
                              textContent: _toDisplayString(_ctx.ec.l10n.localize('shown when qty is over 3'))
                            }, null, 8, ["textContent"]))
                          : _createCommentVNode("", true)
                      ]),
                      _: 1
                    })
                  ]),
                  _: 1
                }))
              : _createCommentVNode("", true),
            ((_ctx.qty <= 3))
              ? (_openBlock(), _createBlock(_component_q_item, {
                  key: 1,
                  clickable: ""
                }, {
                  default: _withCtx(() => [
                    _createVNode(_component_q_item_section, null, {
                      default: _withCtx(() => [
                        ((_ctx.qty <= 3))
                          ? (_openBlock(), _createElementBlock("span", {
                              key: 0,
                              class: "text-inline",
                              textContent: _toDisplayString(_ctx.ec.l10n.localize('hidden when qty is over 3'))
                            }, null, 8, ["textContent"]))
                          : _createCommentVNode("", true)
                      ]),
                      _: 1
                    })
                  ]),
                  _: 1
                }))
              : _createCommentVNode("", true)
          ]),
          _: 1
        })
      ]),
      _: 1
    }),
    _createVNode(_component_m_scope, {
      value: (_ctx.product),
      extra: _ctx.$scope['mqExtra_ProductForm']
    }, {
      default: _withCtx((_ctx) => [
        _createElementVNode("div", { class: "q-mb-md" }, [
          _createVNode(_component_m_form, {
            name: "ProductForm",
            id: "ProductForm",
            action: "/Parity/saveProduct",
            "read-only": "",
            "fields-initial": {'productName':_ctx.$mq.str(_ctx.$scope['productName']),'price':_ctx.$mq.str(_ctx.$scope['price']),'note':_ctx.$mq.str(_ctx.$scope['note']),'moquiFormName':'ProductForm'}
          }, {
            default: _withCtx((formProps) => [
              _createVNode(_component_q_list, {
                bordered: "",
                class: "rounded-borders q-my-sm"
              }, {
                default: _withCtx(() => [
                  _createVNode(_component_q_expansion_item, {
                    dense: "",
                    "expand-separator": "",
                    "switch-toggle-side": "",
                    "header-class": "text-weight-medium",
                    group: "ProductForm_accordion",
                    label: _ctx.ec.l10n.localize('Names')
                  }, {
                    default: _withCtx(() => [
                      _createElementVNode("div", { class: "q-pa-sm" }, [
                        _createElementVNode("div", { class: "q-pa-xs" }, [
                          _createVNode(_component_m_text_line, {
                            id: "ProductForm_productName",
                            type: "text",
                            name: "productName",
                            style: {"width":"100%"},
                            label: _ctx.ec.l10n.localize('Product Name'),
                            modelValue: formProps.fields.productName,
                            "onUpdate:modelValue": $event => ((formProps.fields.productName) = $event),
                            fields: formProps.fields
                          }, null, 8, ["label", "modelValue", "onUpdate:modelValue", "fields"])
                        ])
                      ])
                    ]),
                    _: 2
                  }, 1032, ["label"]),
                  _createVNode(_component_q_expansion_item, {
                    dense: "",
                    "expand-separator": "",
                    "switch-toggle-side": "",
                    "header-class": "text-weight-medium",
                    group: "ProductForm_accordion",
                    label: _ctx.ec.l10n.localize('Amounts'),
                    "default-opened": ""
                  }, {
                    default: _withCtx(() => [
                      _createElementVNode("div", { class: "q-pa-sm" }, [
                        _createElementVNode("div", { class: "q-pa-xs" }, [
                          _createVNode(_component_m_text_line, {
                            id: "ProductForm_price",
                            type: "text",
                            name: "price",
                            style: {"width":"100%"},
                            label: _ctx.ec.l10n.localize('Price'),
                            modelValue: formProps.fields.price,
                            "onUpdate:modelValue": $event => ((formProps.fields.price) = $event),
                            fields: formProps.fields
                          }, null, 8, ["label", "modelValue", "onUpdate:modelValue", "fields"])
                        ]),
                        _createElementVNode("div", { class: "q-pa-xs" }, [
                          _createVNode(_component_m_text_area, {
                            name: "note",
                            id: "ProductForm_note",
                            rows: "3",
                            style: {"width":"100%"},
                            label: _ctx.ec.l10n.localize('Note'),
                            modelValue: formProps.fields.note,
                            "onUpdate:modelValue": $event => ((formProps.fields.note) = $event)
                          }, null, 8, ["label", "modelValue", "onUpdate:modelValue"])
                        ])
                      ])
                    ]),
                    _: 2
                  }, 1032, ["label"])
                ]),
                _: 2
              }, 1024)
            ]),
            _: 1
          }, 8, ["fields-initial"])
        ])
      ]),
      _: 1
    }, 8, ["value", "extra"]),
    _createVNode(_component_m_static_form_list, {
      name: "EditList",
      id: "EditList",
      action: "/Parity/saveProduct",
      "list-name": "itemsForEdit",
      "special-rows": {first:{action:'/Parity/createItem',initial:(_ctx) => ({'itemId':_ctx.$mq.str(_ctx.$scope['itemId']),'name':_ctx.$mq.str(_ctx.$scope['name']),'secret':_ctx.$mq.str(_ctx.$mq.dflt(_ctx.$scope['secret'],'S1'))})},last:{action:null,initial:(_ctx) => ({'name':_ctx.$mq.str(_ctx.$scope['name']),'name_display':_ctx.ec.l10n.localize('the last row')})}}
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
          _createVNode(_component_m_form, {
            name: 'EditList_'+_ctx.rowIndex,
            id: 'EditList_'+_ctx.rowIndex,
            action: "/Parity/saveProduct",
            "fields-initial": {'itemId':_ctx.$mq.str(_ctx.$scope['itemId']),'itemId_display':_ctx.$mq.str(_ctx.$scope['itemId']),'name':_ctx.$mq.str(_ctx.$scope['name']),'secret':_ctx.$mq.str(_ctx.$scope['secret']),'moquiFormName':'EditList','pageIndex':_ctx.$mq.str(_ctx.$scope['pageIndex']||0)}
          }, {
            default: _withCtx((formProps) => [
              _createElementVNode("div", { class: "td" }, [
                _createElementVNode("div", { class: "form-group" }, [
                  _createVNode(_component_m_display, {
                    name: "itemId",
                    id: 'EditList_itemId'+'_'+_ctx.rowIndex+'_display',
                    modelValue: formProps.fields.itemId,
                    "onUpdate:modelValue": $event => ((formProps.fields.itemId) = $event),
                    display: formProps.fields.itemId_display,
                    fields: formProps.fields
                  }, null, 8, ["id", "modelValue", "onUpdate:modelValue", "display", "fields"])
                ])
              ]),
              _createElementVNode("div", { class: "td" }, [
                _createElementVNode("div", { class: "form-group" }, [
                  _createVNode(_component_m_text_line, {
                    id: 'EditList_name'+'_'+_ctx.rowIndex,
                    type: "text",
                    name: "name",
                    size: "20",
                    label: _ctx.ec.l10n.localize('Name'),
                    modelValue: formProps.fields.name,
                    "onUpdate:modelValue": $event => ((formProps.fields.name) = $event),
                    fields: formProps.fields
                  }, null, 8, ["id", "label", "modelValue", "onUpdate:modelValue", "fields"])
                ])
              ]),
              _createElementVNode("div", { class: "td" }, [
                _createElementVNode("div", { class: "form-group" }, [
                  _createVNode(_component_q_btn, {
                    dense: "",
                    outline: !formProps.hasFieldsChanged,
                    unelevated: formProps.hasFieldsChanged,
                    "no-caps": "",
                    type: "submit",
                    name: "go",
                    value: "go",
                    color: "primary",
                    id: 'EditList_go'+'_'+_ctx.rowIndex
                  }, {
                    default: _withCtx(() => [
                      _createElementVNode("span", {
                        textContent: _toDisplayString(_ctx.ec.l10n.localize('Save'))
                      }, null, 8, ["textContent"])
                    ]),
                    _: 1
                  }, 8, ["outline", "unelevated", "id"])
                ])
              ])
            ]),
            _: 1
          }, 8, ["name", "id", "fields-initial"])
        ], 2)
      ]),
      row_first: _withCtx((_ctx) => [
        _createElementVNode("div", { class: "tr first" }, [
          _createElementVNode("div", { class: "td" }, [
            _createElementVNode("div", { class: "form-group" }, [
              _createVNode(_component_m_text_line, {
                id: "EditList_first_itemId",
                type: "text",
                name: "itemId",
                size: "10",
                label: _ctx.ec.l10n.localize('Item ID'),
                modelValue: _ctx.formProps.fields.itemId,
                "onUpdate:modelValue": $event => ((_ctx.formProps.fields.itemId) = $event),
                fields: _ctx.formProps.fields
              }, null, 8, ["label", "modelValue", "onUpdate:modelValue", "fields"])
            ])
          ]),
          _createElementVNode("div", { class: "td" }, [
            _createElementVNode("div", { class: "form-group" }, [
              _createVNode(_component_m_text_line, {
                id: "EditList_first_name",
                type: "text",
                name: "name",
                size: "20",
                label: _ctx.ec.l10n.localize('Name'),
                modelValue: _ctx.formProps.fields.name,
                "onUpdate:modelValue": $event => ((_ctx.formProps.fields.name) = $event),
                fields: _ctx.formProps.fields
              }, null, 8, ["label", "modelValue", "onUpdate:modelValue", "fields"])
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
                id: "EditList_first_go"
              }, {
                default: _withCtx(() => [
                  _createElementVNode("span", {
                    textContent: _toDisplayString(_ctx.ec.l10n.localize('Create'))
                  }, null, 8, ["textContent"])
                ]),
                _: 1
              }, 8, ["outline", "unelevated"])
            ])
          ])
        ])
      ]),
      row_last: _withCtx((_ctx) => [
        _createElementVNode("div", { class: "tr last" }, [
          _createElementVNode("div", { class: "td" }),
          _createElementVNode("div", { class: "td" }, [
            _createElementVNode("div", { class: "form-group" }, [
              _createVNode(_component_m_display, {
                name: "name",
                id: "EditList_last_name_display"
              }, {
                default: _withCtx(() => [
                  _createElementVNode("span", {
                    textContent: _toDisplayString(_ctx.ec.l10n.localize('the last row'))
                  }, null, 8, ["textContent"])
                ]),
                _: 1
              }),
              _createElementVNode("input", {
                type: "hidden",
                name: "name",
                value: _ctx.$mq.str(_ctx.$scope['name'])
              }, null, 8, ["value"])
            ])
          ]),
          _createElementVNode("div", { class: "td" })
        ])
      ]),
      _: 1
    }, 8, ["special-rows"]),
    _createVNode(_component_m_static_form_list, {
      name: "PickList",
      id: "PickList",
      action: "/Parity",
      "list-name": "itemsForEdit",
      "skip-form": "",
      "selection-id-field": "itemId",
      "selection-list-mode": false
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
        ])
      ]),
      row: _withCtx((_ctx) => [
        _createElementVNode("div", {
          class: _normalizeClass(["tr", {'text-weight-bold': _ctx._moquiRowType === 'total'}])
        }, [
          _createElementVNode("div", { class: "td" }, [
            (_ctx.$mq.str(_ctx.$scope['itemId']))
              ? (_openBlock(), _createBlock(_component_q_checkbox, {
                  key: 0,
                  dense: "",
                  size: "sm",
                  "model-value": !!_ctx.mqSel.checkboxStates[_ctx.rowIndex],
                  "onUpdate:modelValue": (value, event) => { _ctx.mqSel.clickCheckbox(event || {}, _ctx.rowIndex); _ctx.mqSel.checkboxStates[_ctx.rowIndex] = value; }
                }, null, 8, ["model-value", "onUpdate:modelValue"]))
              : _createCommentVNode("", true)
          ]),
          _createElementVNode("div", { class: "td" }, [
            _createElementVNode("div", { class: "form-group" }, [
              _createVNode(_component_m_display, {
                name: "itemId",
                id: 'PickList_itemId'+'_'+_ctx.rowIndex+'_display'
              }, {
                default: _withCtx(() => [
                  _createElementVNode("span", {
                    textContent: _toDisplayString(_ctx.$mq.str(_ctx.$scope['itemId']))
                  }, null, 8, ["textContent"])
                ]),
                _: 1
              }, 8, ["id"]),
              _createElementVNode("input", {
                type: "hidden",
                name: "itemId",
                value: _ctx.$mq.str(_ctx.$scope['itemId'])
              }, null, 8, ["value"])
            ])
          ]),
          _createElementVNode("div", { class: "td" }, [
            _createElementVNode("div", { class: "form-group" }, [
              _createVNode(_component_m_display, {
                name: "name",
                id: 'PickList_name'+'_'+_ctx.rowIndex+'_display'
              }, {
                default: _withCtx(() => [
                  _createElementVNode("span", {
                    textContent: _toDisplayString(_ctx.$mq.str(_ctx.$scope['name']))
                  }, null, 8, ["textContent"])
                ]),
                _: 1
              }, 8, ["id"]),
              _createElementVNode("input", {
                type: "hidden",
                name: "name",
                value: _ctx.$mq.str(_ctx.$scope['name'])
              }, null, 8, ["value"])
            ])
          ])
        ], 2)
      ]),
      selection: _withCtx((mqSelSet) => [
        _createVNode(_component_q_card, { flat: "" }, {
          default: _withCtx(() => [
            _createVNode(_component_q_card_section, {
              horizontal: "",
              class: "q-pa-sm"
            }, {
              default: _withCtx(() => [
                _createElementVNode("span", { class: "q-my-md" }, [
                  _createElementVNode("div", { class: "q-ma-sm" }, [
                    _createVNode(_component_m_container_dialog, {
                      color: "primary",
                      "button-text": _ctx.ec.l10n.localize('Archive'),
                      title: _ctx.ec.l10n.localize('Archive Items')
                    }, {
                      default: _withCtx(() => [
                        _createVNode(_component_m_scope, {
                          value: (_ctx.fieldValues),
                          extra: _ctx.$scope['mqExtra_ArchiveForm']
                        }, {
                          default: _withCtx((_ctx) => [
                            _createElementVNode("div", { class: "q-mb-md" }, [
                              _createVNode(_component_m_form, {
                                name: "ArchiveForm",
                                id: "ArchiveForm",
                                action: "/Parity/archiveItems",
                                "read-only": "",
                                "fields-initial": {'reason':_ctx.$mq.str(_ctx.$scope['reason']),'moquiFormName':'ArchiveForm'},
                                "parent-checkbox-set": mqSelSet
                              }, {
                                default: _withCtx((formProps) => [
                                  _createElementVNode("div", { class: "q-pa-xs" }, [
                                    _createVNode(_component_m_text_line, {
                                      id: "ArchiveForm_reason",
                                      type: "text",
                                      name: "reason",
                                      size: "20",
                                      label: _ctx.ec.l10n.localize('Reason'),
                                      modelValue: formProps.fields.reason,
                                      "onUpdate:modelValue": $event => ((formProps.fields.reason) = $event),
                                      fields: formProps.fields
                                    }, null, 8, ["label", "modelValue", "onUpdate:modelValue", "fields"])
                                  ]),
                                  _createElementVNode("div", { class: "q-pa-xs" }, [
                                    _createVNode(_component_q_btn, {
                                      dense: "",
                                      outline: !formProps.hasFieldsChanged,
                                      unelevated: formProps.hasFieldsChanged,
                                      "no-caps": "",
                                      type: "submit",
                                      name: "go",
                                      value: "go",
                                      color: "primary",
                                      id: "ArchiveForm_go"
                                    }, {
                                      default: _withCtx(() => [
                                        _createElementVNode("span", {
                                          textContent: _toDisplayString(_ctx.ec.l10n.localize('Archive'))
                                        }, null, 8, ["textContent"])
                                      ]),
                                      _: 1
                                    }, 8, ["outline", "unelevated"])
                                  ])
                                ]),
                                _: 1
                              }, 8, ["fields-initial", "parent-checkbox-set"])
                            ])
                          ]),
                          _: 2
                        }, 1032, ["value", "extra"])
                      ]),
                      _: 2
                    }, 1032, ["button-text", "title"])
                  ])
                ])
              ]),
              _: 2
            }, 1024)
          ]),
          _: 2
        }, 1024)
      ]),
      _: 1
    }),
    _createVNode(_component_m_static_form_list, {
      name: "OrderList",
      id: "OrderList",
      action: "/Parity",
      "list-name": "orderLines",
      "skip-form": ""
    }, {
      header: _withCtx((hs) => [
        _createElementVNode("div", { class: "th text-left" }, [
          _createElementVNode("div", null, [
            _createElementVNode("div", { class: "form-title" }, [
              _createElementVNode("span", {
                textContent: _toDisplayString(_ctx.ec.l10n.localize('Order ID'))
              }, null, 8, ["textContent"])
            ])
          ])
        ]),
        _createElementVNode("div", { class: "th text-left" }, [
          _createElementVNode("div", null, [
            _createElementVNode("div", { class: "form-title" }, [
              _createElementVNode("span", {
                textContent: _toDisplayString(_ctx.ec.l10n.localize('Customer'))
              }, null, 8, ["textContent"])
            ])
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
                name: "orderId",
                id: 'OrderList_orderId'+'_'+_ctx.rowIndex+'_display'
              }, {
                default: _withCtx(() => [
                  _createElementVNode("span", {
                    textContent: _toDisplayString(_ctx.$mq.str(_ctx.$scope['orderId']))
                  }, null, 8, ["textContent"])
                ]),
                _: 1
              }, 8, ["id"]),
              _createElementVNode("input", {
                type: "hidden",
                name: "orderId",
                value: _ctx.$mq.str(_ctx.$scope['orderId'])
              }, null, 8, ["value"])
            ])
          ]),
          _createElementVNode("div", { class: "td" }, [
            _createElementVNode("div", { class: "form-group" }, [
              _createVNode(_component_m_display, {
                name: "customer",
                id: 'OrderList_customer'+'_'+_ctx.rowIndex+'_display'
              }, {
                default: _withCtx(() => [
                  _createElementVNode("span", {
                    textContent: _toDisplayString(_ctx.$mq.str(_ctx.$scope['customer']))
                  }, null, 8, ["textContent"])
                ]),
                _: 1
              }, 8, ["id"]),
              _createElementVNode("input", {
                type: "hidden",
                name: "customer",
                value: _ctx.$mq.str(_ctx.$scope['customer'])
              }, null, 8, ["value"])
            ])
          ])
        ], 2)
      ]),
      subhead: _withCtx(() => [
        _createElementVNode("div", { class: "col" }, [
          _createElementVNode("span", {
            textContent: _toDisplayString(_ctx.ec.l10n.localize('Item'))
          }, null, 8, ["textContent"])
        ]),
        _createElementVNode("div", { class: "col" }, [
          _createElementVNode("span", {
            textContent: _toDisplayString(_ctx.ec.l10n.localize('Qty'))
          }, null, 8, ["textContent"])
        ])
      ]),
      subrow: _withCtx((_ctx) => [
        _createElementVNode("div", { class: "col" }, [
          _createElementVNode("div", { class: "form-group" }, [
            _createVNode(_component_m_display, {
              name: "item",
              id: 'OrderList_item'+'_'+_ctx.rowIndex+'_display'
            }, {
              default: _withCtx(() => [
                _createElementVNode("span", {
                  textContent: _toDisplayString(_ctx.$mq.str(_ctx.$scope['item']))
                }, null, 8, ["textContent"])
              ]),
              _: 1
            }, 8, ["id"]),
            _createElementVNode("input", {
              type: "hidden",
              name: "item",
              value: _ctx.$mq.str(_ctx.$scope['item'])
            }, null, 8, ["value"])
          ])
        ]),
        _createElementVNode("div", { class: "col" }, [
          _createElementVNode("div", { class: "form-group" }, [
            _createVNode(_component_m_display, {
              name: "qty",
              id: 'OrderList_qty'+'_'+_ctx.rowIndex+'_display'
            }, {
              default: _withCtx(() => [
                _createElementVNode("span", {
                  textContent: _toDisplayString(_ctx.$mq.str(_ctx.$scope['qty']))
                }, null, 8, ["textContent"])
              ]),
              _: 1
            }, 8, ["id"]),
            _createElementVNode("input", {
              type: "hidden",
              name: "qty",
              value: _ctx.$mq.str(_ctx.$scope['qty'])
            }, null, 8, ["value"])
          ])
        ])
      ]),
      _: 1
    }),
    (_ctx.$scope['linesCount'])
      ? (_openBlock(), _createBlock(_component_m_form_paginate, {
          key: 4,
          paginate: {count:Number(_ctx.$scope['linesCount']),pageIndex:Number(_ctx.$scope['linesPageIndex']),pageSize:Number(_ctx.$scope['linesPageSize']),pageMaxIndex:Number(_ctx.$scope['linesPageMaxIndex']),pageRangeLow:Number(_ctx.$scope['linesPageRangeLow']),pageRangeHigh:Number(_ctx.$scope['linesPageRangeHigh'])}
        }, null, 8, ["paginate"]))
      : _createCommentVNode("", true),
    (_openBlock(true), _createElementBlock(_Fragment, null, _renderList((_ctx.lines), (line, line_index) => {
      return (_openBlock(), _createElementBlock("div", {
        key: line_index,
        class: "text-inline",
        textContent: _toDisplayString(_ctx.ec.resource.expand('line ${line.lineId}',{'line.lineId':()=>(line.lineId)}))
      }, null, 8, ["textContent"]))
    }), 128))
  ], 64))
}
};})();

/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

// Generated from the component templates of WebrootVue.qvt.js; do not edit. Make it again with
//   UPDATE_GOLDEN=1 cargo test -p moqui-core --lib client_components_are_precompiled
// qvt.applyRenders uses an entry only while its hash matches the template (plans/archive/QvtServerStatic.md, Page build).
(function (moqui) {
    "use strict";
    moqui.qvtRenders = {
        "m-box-body": { hash: "4645e5c2", make: function (Vue) {
            const { renderSlot: _renderSlot, normalizeStyle: _normalizeStyle, openBlock: _openBlock, createElementBlock: _createElementBlock } = Vue

            return function render(_ctx, _cache) {
              return (_openBlock(), _createElementBlock("div", {
                class: "q-pa-xs",
                style: _normalizeStyle(_ctx.bodyStyle)
              }, [
                _renderSlot(_ctx.$slots, "default")
              ], 4))
            }
        } },
        "m-chart": { hash: "e1d8e95a", make: function (Vue) {
            const { createElementVNode: _createElementVNode, normalizeStyle: _normalizeStyle, openBlock: _openBlock, createElementBlock: _createElementBlock } = Vue

            return function render(_ctx, _cache) {
              return (_openBlock(), _createElementBlock("div", {
                class: "chart-container",
                style: _normalizeStyle([{"position":"relative"}, {height:_ctx.height,width:_ctx.width}])
              }, [
                _createElementVNode("canvas", { ref: "canvas" }, null, 512)
              ], 4))
            }
        } },
        "m-check": { hash: "9f5f3c3", make: function (Vue) {
            const { resolveComponent: _resolveComponent, openBlock: _openBlock, createBlock: _createBlock, createCommentVNode: _createCommentVNode, toDisplayString: _toDisplayString, createTextVNode: _createTextVNode, withCtx: _withCtx, renderList: _renderList, Fragment: _Fragment, createElementBlock: _createElementBlock, mergeProps: _mergeProps } = Vue

            return function render(_ctx, _cache) {
              const _component_q_checkbox = _resolveComponent("q-checkbox")
              const _component_q_tooltip = _resolveComponent("q-tooltip")
              const _component_m_field = _resolveComponent("m-field")

              return (!_ctx.options.length)
                ? (_openBlock(), _createBlock(_component_q_checkbox, {
                    key: 0,
                    size: "xs",
                    "model-value": _ctx.local,
                    "onUpdate:modelValue": _ctx.onInput,
                    "true-value": _ctx.trueValue,
                    "false-value": _ctx.falseValue,
                    label: _ctx.singleLabel,
                    name: _ctx.name,
                    id: _ctx.id,
                    form: _ctx.form,
                    disable: _ctx.disable
                  }, null, 8, ["model-value", "onUpdate:modelValue", "true-value", "false-value", "label", "name", "id", "form", "disable"]))
                : (!_ctx.noWrapper)
                  ? (_openBlock(), _createBlock(_component_m_field, _mergeProps({
                      key: 1,
                      label: _ctx.label,
                      "bg-color": _ctx.bg,
                      disable: _ctx.disable
                    }, _ctx.errorAttrs), {
                      default: _withCtx(() => [
                        (_ctx.tooltip)
                          ? (_openBlock(), _createBlock(_component_q_tooltip, { key: 0 }, {
                              default: _withCtx(() => [
                                _createTextVNode(_toDisplayString(_ctx.tooltip), 1)
                              ]),
                              _: 1
                            }))
                          : _createCommentVNode("", true),
                        (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.options, (option, index) => {
                          return (_openBlock(), _createBlock(_component_q_checkbox, {
                            key: option.value,
                            size: "xs",
                            val: option.value,
                            label: option.label,
                            "model-value": _ctx.local,
                            "onUpdate:modelValue": _ctx.onInput,
                            name: _ctx.name,
                            id: index > 0 ? _ctx.id + '_' + index : _ctx.id,
                            form: _ctx.form,
                            disable: _ctx.disable
                          }, null, 8, ["val", "label", "model-value", "onUpdate:modelValue", "name", "id", "form", "disable"]))
                        }), 128))
                      ]),
                      _: 1
                    }, 16, ["label", "bg-color", "disable"]))
                  : (_openBlock(true), _createElementBlock(_Fragment, { key: 2 }, _renderList(_ctx.options, (option, index) => {
                      return (_openBlock(), _createBlock(_component_q_checkbox, {
                        key: option.value,
                        size: "xs",
                        val: option.value,
                        label: option.label,
                        "model-value": _ctx.local,
                        "onUpdate:modelValue": _ctx.onInput,
                        name: _ctx.name,
                        id: index > 0 ? _ctx.id + '_' + index : _ctx.id,
                        form: _ctx.form,
                        disable: _ctx.disable
                      }, null, 8, ["val", "label", "model-value", "onUpdate:modelValue", "name", "id", "form", "disable"]))
                    }), 128))
            }
        } },
        "m-checkbox-set": { hash: "94e2c135", make: function (Vue) {
            const { renderSlot: _renderSlot, openBlock: _openBlock, createElementBlock: _createElementBlock } = Vue

            return function render(_ctx, _cache) {
              return (_openBlock(), _createElementBlock("span", { class: "checkbox-set" }, [
                _renderSlot(_ctx.$slots, "default", {
                  checkboxAllState: _ctx.checkboxAllState,
                  setCheckboxAllState: _ctx.setCheckboxAllState,
                  checkboxStates: _ctx.checkboxStates,
                  clickCheckbox: _ctx.clickCheckbox,
                  addCheckboxParameters: _ctx.addCheckboxParameters
                })
              ]))
            }
        } },
        "m-container-box": { hash: "18167a74", make: function (Vue) {
            const { toDisplayString: _toDisplayString, normalizeClass: _normalizeClass, openBlock: _openBlock, createElementBlock: _createElementBlock, createCommentVNode: _createCommentVNode, renderSlot: _renderSlot, resolveComponent: _resolveComponent, createVNode: _createVNode, withModifiers: _withModifiers, withCtx: _withCtx, vShow: _vShow, createElementVNode: _createElementVNode, withDirectives: _withDirectives, createBlock: _createBlock } = Vue

            return function render(_ctx, _cache) {
              const _component_q_space = _resolveComponent("q-space")
              const _component_q_btn = _resolveComponent("q-btn")
              const _component_q_card_actions = _resolveComponent("q-card-actions")
              const _component_q_card_section = _resolveComponent("q-card-section")
              const _component_q_card = _resolveComponent("q-card")

              return (_openBlock(), _createBlock(_component_q_card, {
                flat: "",
                bordered: "",
                class: "q-ma-sm m-container-box"
              }, {
                default: _withCtx(() => [
                  _createVNode(_component_q_card_actions, {
                    onClick: _withModifiers(_ctx.toggleBody, ["self"])
                  }, {
                    default: _withCtx(() => [
                      (_ctx.title && _ctx.title.length)
                        ? (_openBlock(), _createElementBlock("h5", {
                            key: 0,
                            onClick: _ctx.toggleBody,
                            class: _normalizeClass('text-' + _ctx.type)
                          }, _toDisplayString(_ctx.title), 11, ["onClick"]))
                        : _createCommentVNode("", true),
                      _renderSlot(_ctx.$slots, "header"),
                      _createVNode(_component_q_space),
                      _renderSlot(_ctx.$slots, "toolbar"),
                      _createVNode(_component_q_btn, {
                        color: "grey",
                        round: "",
                        flat: "",
                        dense: "",
                        icon: _ctx.isBodyOpen ? 'keyboard_arrow_up' : 'keyboard_arrow_down',
                        onClick: _ctx.toggleBody
                      }, null, 8, ["icon", "onClick"])
                    ]),
                    _: 3
                  }, 8, ["onClick"]),
                  _withDirectives(_createElementVNode("div", null, [
                    _createVNode(_component_q_card_section, null, {
                      default: _withCtx(() => [
                        _renderSlot(_ctx.$slots, "default")
                      ]),
                      _: 3
                    })
                  ], 512), [
                    [_vShow, _ctx.isBodyOpen]
                  ])
                ]),
                _: 3
              }))
            }
        } },
        "m-container-dialog": { hash: "e84735b5", make: function (Vue) {
            const { renderSlot: _renderSlot, resolveComponent: _resolveComponent, normalizeClass: _normalizeClass, createVNode: _createVNode, createElementVNode: _createElementVNode, withCtx: _withCtx, openBlock: _openBlock, createElementBlock: _createElementBlock } = Vue

            return function render(_ctx, _cache) {
              const _component_q_btn = _resolveComponent("q-btn")
              const _component_m_dialog = _resolveComponent("m-dialog")

              return (_openBlock(), _createElementBlock("span", null, [
                _createElementVNode("span", {
                  onClick: $event => (_ctx.show())
                }, [
                  _renderSlot(_ctx.$slots, "button", {}, () => [
                    _createVNode(_component_q_btn, {
                      dense: "",
                      outline: "",
                      "no-caps": "",
                      icon: _ctx.buttonIcon,
                      label: _ctx.buttonText,
                      color: _ctx.color,
                      class: _normalizeClass(_ctx.buttonClass)
                    }, null, 8, ["icon", "label", "color", "class"])
                  ])
                ], 8, ["onClick"]),
                _createVNode(_component_m_dialog, {
                  modelValue: _ctx.isShown,
                  "onUpdate:modelValue": $event => ((_ctx.isShown) = $event),
                  id: _ctx.id,
                  title: _ctx.title,
                  color: _ctx.color,
                  width: _ctx.width
                }, {
                  default: _withCtx(() => [
                    _renderSlot(_ctx.$slots, "default")
                  ]),
                  _: 3
                }, 8, ["modelValue", "onUpdate:modelValue", "id", "title", "color", "width"])
              ]))
            }
        } },
        "m-date-period": { hash: "5e45dbc3", make: function (Vue) {
            const { resolveComponent: _resolveComponent, createVNode: _createVNode, createTextVNode: _createTextVNode, withCtx: _withCtx, openBlock: _openBlock, createElementBlock: _createElementBlock, createCommentVNode: _createCommentVNode, toDisplayString: _toDisplayString, createBlock: _createBlock } = Vue

            return function render(_ctx, _cache) {
              const _component_m_date_time = _resolveComponent("m-date-time")
              const _component_q_icon = _resolveComponent("q-icon")
              const _component_q_tooltip = _resolveComponent("q-tooltip")
              const _component_q_btn = _resolveComponent("q-btn")
              const _component_q_select = _resolveComponent("q-select")
              const _component_q_date = _resolveComponent("q-date")
              const _component_q_popup_proxy = _resolveComponent("q-popup-proxy")
              const _component_q_input = _resolveComponent("q-input")

              return (_ctx.fromThruMode)
                ? (_openBlock(), _createElementBlock("div", {
                    key: 0,
                    class: "row"
                  }, [
                    _createVNode(_component_m_date_time, {
                      name: _ctx.name+'_from',
                      id: _ctx.id+'_from',
                      label: _ctx.label+' From',
                      form: _ctx.form,
                      type: _ctx.fromThruType,
                      modelValue: _ctx.fields[_ctx.name+'_from'],
                      "onUpdate:modelValue": $event => ((_ctx.fields[_ctx.name+'_from']) = $event)
                    }, null, 8, ["name", "id", "label", "form", "type", "modelValue", "onUpdate:modelValue"]),
                    _createVNode(_component_q_icon, {
                      class: "q-my-auto",
                      name: "remove"
                    }),
                    _createVNode(_component_m_date_time, {
                      name: _ctx.name+'_thru',
                      id: _ctx.id+'_thru',
                      label: _ctx.label+' Thru',
                      form: _ctx.form,
                      type: _ctx.fromThruType,
                      modelValue: _ctx.fields[_ctx.name+'_thru'],
                      "onUpdate:modelValue": $event => ((_ctx.fields[_ctx.name+'_thru']) = $event)
                    }, {
                      after: _withCtx(() => [
                        _createVNode(_component_q_btn, {
                          dense: "",
                          flat: "",
                          icon: "calendar_view_day",
                          onClick: _ctx.toggleMode
                        }, {
                          default: _withCtx(() => [
                            _createVNode(_component_q_tooltip, null, {
                              default: _withCtx(() => [
                                _createTextVNode("Period Select Mode")
                              ]),
                              _: 1
                            })
                          ]),
                          _: 1
                        }, 8, ["onClick"]),
                        _createVNode(_component_q_btn, {
                          dense: "",
                          flat: "",
                          icon: "clear",
                          onClick: _ctx.clearAll
                        }, {
                          default: _withCtx(() => [
                            _createVNode(_component_q_tooltip, null, {
                              default: _withCtx(() => [
                                _createTextVNode("Clear")
                              ]),
                              _: 1
                            })
                          ]),
                          _: 1
                        }, 8, ["onClick"])
                      ]),
                      _: 1
                    }, 8, ["name", "id", "label", "form", "type", "modelValue", "onUpdate:modelValue"])
                  ]))
                : (_openBlock(), _createElementBlock("div", {
                    key: 1,
                    class: "row"
                  }, [
                    _createVNode(_component_q_input, {
                      dense: "",
                      outlined: "",
                      "stack-label": "",
                      label: _ctx.label,
                      modelValue: _ctx.fields[_ctx.name+'_pdate'],
                      "onUpdate:modelValue": $event => ((_ctx.fields[_ctx.name+'_pdate']) = $event),
                      mask: "####-##-##",
                      "fill-mask": "",
                      for: _ctx.id,
                      name: _ctx.name+'_pdate',
                      form: _ctx.form,
                      style: {"max-width":"max-content"},
                      "bg-color": _ctx.changed(_ctx.name+'_pdate')
                    }, {
                      before: _withCtx(() => [
                        _createVNode(_component_q_select, {
                          class: "q-pr-xs",
                          dense: "",
                          outlined: "",
                          "options-dense": "",
                          "emit-value": "",
                          "map-options": "",
                          modelValue: _ctx.fields[_ctx.name+'_poffset'],
                          "onUpdate:modelValue": $event => ((_ctx.fields[_ctx.name+'_poffset']) = $event),
                          name: _ctx.name+'_poffset',
                          "bg-color": _ctx.changed(_ctx.name+'_poffset'),
                          "stack-label": "",
                          label: "Offset",
                          options: _ctx.dateOffsets,
                          form: _ctx.form,
                          behavior: "menu"
                        }, null, 8, ["modelValue", "onUpdate:modelValue", "name", "bg-color", "options", "form"]),
                        _createVNode(_component_q_select, {
                          dense: "",
                          outlined: "",
                          "options-dense": "",
                          "emit-value": "",
                          "map-options": "",
                          modelValue: _ctx.fields[_ctx.name+'_period'],
                          "onUpdate:modelValue": $event => ((_ctx.fields[_ctx.name+'_period']) = $event),
                          name: _ctx.name+'_period',
                          "bg-color": _ctx.changed(_ctx.name+'_period'),
                          "stack-label": "",
                          label: "Period",
                          options: _ctx.datePeriods,
                          form: _ctx.form,
                          behavior: "menu"
                        }, null, 8, ["modelValue", "onUpdate:modelValue", "name", "bg-color", "options", "form"])
                      ]),
                      prepend: _withCtx(() => [
                        _createVNode(_component_q_icon, {
                          name: "event",
                          class: "cursor-pointer"
                        }, {
                          default: _withCtx(() => [
                            _createVNode(_component_q_popup_proxy, {
                              ref: "qDateProxy",
                              "transition-show": "scale",
                              "transition-hide": "scale"
                            }, {
                              default: _withCtx(() => [
                                _createVNode(_component_q_date, {
                                  modelValue: _ctx.fields[_ctx.name+'_pdate'],
                                  "onUpdate:modelValue": [$event => ((_ctx.fields[_ctx.name+'_pdate']) = $event), _ctx.hideDate],
                                  mask: "YYYY-MM-DD"
                                }, null, 8, ["modelValue", "onUpdate:modelValue"])
                              ]),
                              _: 1
                            }, 512)
                          ]),
                          _: 1
                        })
                      ]),
                      after: _withCtx(() => [
                        _createVNode(_component_q_btn, {
                          dense: "",
                          flat: "",
                          icon: "date_range",
                          onClick: _ctx.toggleMode
                        }, {
                          default: _withCtx(() => [
                            _createVNode(_component_q_tooltip, null, {
                              default: _withCtx(() => [
                                _createTextVNode("Date Range Mode")
                              ]),
                              _: 1
                            })
                          ]),
                          _: 1
                        }, 8, ["onClick"]),
                        _createVNode(_component_q_btn, {
                          dense: "",
                          flat: "",
                          icon: "clear",
                          onClick: _ctx.clearAll
                        }, {
                          default: _withCtx(() => [
                            _createVNode(_component_q_tooltip, null, {
                              default: _withCtx(() => [
                                _createTextVNode("Clear")
                              ]),
                              _: 1
                            })
                          ]),
                          _: 1
                        }, 8, ["onClick"])
                      ]),
                      default: _withCtx(() => [
                        (_ctx.tooltip)
                          ? (_openBlock(), _createBlock(_component_q_tooltip, { key: 0 }, {
                              default: _withCtx(() => [
                                _createTextVNode(_toDisplayString(_ctx.tooltip), 1)
                              ]),
                              _: 1
                            }))
                          : _createCommentVNode("", true)
                      ]),
                      _: 1
                    }, 8, ["label", "modelValue", "onUpdate:modelValue", "for", "name", "form", "bg-color"])
                  ]))
            }
        } },
        "m-date-time": { hash: "f3d52fdc", make: function (Vue) {
            const { resolveComponent: _resolveComponent, createVNode: _createVNode, withCtx: _withCtx, renderSlot: _renderSlot, toDisplayString: _toDisplayString, createTextVNode: _createTextVNode, openBlock: _openBlock, createBlock: _createBlock, createCommentVNode: _createCommentVNode, mergeProps: _mergeProps, createSlots: _createSlots } = Vue

            return function render(_ctx, _cache) {
              const _component_q_date = _resolveComponent("q-date")
              const _component_q_popup_proxy = _resolveComponent("q-popup-proxy")
              const _component_q_icon = _resolveComponent("q-icon")
              const _component_q_time = _resolveComponent("q-time")
              const _component_q_tooltip = _resolveComponent("q-tooltip")
              const _component_q_input = _resolveComponent("q-input")

              return (_openBlock(), _createBlock(_component_q_input, _mergeProps({
                dense: "",
                outlined: "",
                "stack-label": "",
                label: _ctx.label,
                "model-value": _ctx.local,
                "onUpdate:modelValue": _ctx.onInput,
                rules: _ctx.allRules,
                mask: _ctx.inputMask,
                "fill-mask": "",
                for: _ctx.id,
                name: _ctx.name,
                form: _ctx.form,
                disable: _ctx.disable,
                size: _ctx.sizeVal,
                style: {"max-width":"max-content"},
                "bg-color": _ctx.bg
              }, _ctx.errorAttrs), _createSlots({
                after: _withCtx(() => [
                  _renderSlot(_ctx.$slots, "after")
                ]),
                default: _withCtx(() => [
                  (_ctx.tooltip)
                    ? (_openBlock(), _createBlock(_component_q_tooltip, { key: 0 }, {
                        default: _withCtx(() => [
                          _createTextVNode(_toDisplayString(_ctx.tooltip), 1)
                        ]),
                        _: 1
                      }))
                    : _createCommentVNode("", true)
                ]),
                _: 2
              }, [
                (_ctx.hasDate)
                  ? {
                      name: "prepend",
                      fn: _withCtx(() => [
                        _createVNode(_component_q_icon, {
                          name: "event",
                          class: "cursor-pointer"
                        }, {
                          default: _withCtx(() => [
                            _createVNode(_component_q_popup_proxy, {
                              ref: "qDateProxy",
                              "transition-show": "scale",
                              "transition-hide": "scale"
                            }, {
                              default: _withCtx(() => [
                                _createVNode(_component_q_date, {
                                  "model-value": _ctx.local,
                                  "onUpdate:modelValue": _ctx.onDate,
                                  mask: _ctx.formatVal
                                }, null, 8, ["model-value", "onUpdate:modelValue", "mask"])
                              ]),
                              _: 1
                            }, 512)
                          ]),
                          _: 1
                        })
                      ]),
                      key: "0"
                    }
                  : undefined,
                (_ctx.hasTime)
                  ? {
                      name: "append",
                      fn: _withCtx(() => [
                        _createVNode(_component_q_icon, {
                          name: "access_time",
                          class: "cursor-pointer"
                        }, {
                          default: _withCtx(() => [
                            _createVNode(_component_q_popup_proxy, {
                              ref: "qTimeProxy",
                              "transition-show": "scale",
                              "transition-hide": "scale"
                            }, {
                              default: _withCtx(() => [
                                _createVNode(_component_q_time, {
                                  "model-value": _ctx.local,
                                  "onUpdate:modelValue": _ctx.onTime,
                                  mask: _ctx.formatVal,
                                  format24h: ""
                                }, null, 8, ["model-value", "onUpdate:modelValue", "mask"])
                              ]),
                              _: 1
                            }, 512)
                          ]),
                          _: 1
                        })
                      ]),
                      key: "1"
                    }
                  : undefined
              ]), 1040, ["label", "model-value", "onUpdate:modelValue", "rules", "mask", "for", "name", "form", "disable", "size", "bg-color"]))
            }
        } },
        "m-dialog": { hash: "bbd0a875", make: function (Vue) {
            const { toDisplayString: _toDisplayString, createElementVNode: _createElementVNode, resolveComponent: _resolveComponent, createVNode: _createVNode, resolveDirective: _resolveDirective, withDirectives: _withDirectives, normalizeStyle: _normalizeStyle, withCtx: _withCtx, renderSlot: _renderSlot, openBlock: _openBlock, createBlock: _createBlock } = Vue

            return function render(_ctx, _cache) {
              const _component_q_space = _resolveComponent("q-space")
              const _component_q_btn = _resolveComponent("q-btn")
              const _component_q_card_actions = _resolveComponent("q-card-actions")
              const _component_q_separator = _resolveComponent("q-separator")
              const _component_q_card_section = _resolveComponent("q-card-section")
              const _component_q_card = _resolveComponent("q-card")
              const _component_q_dialog = _resolveComponent("q-dialog")
              const _directive_close_popup = _resolveDirective("close-popup")

              return (_openBlock(), _createBlock(_component_q_dialog, {
                "model-value": _ctx.modelValue,
                "onUpdate:modelValue": $event => (_ctx.$emit('update:modelValue', $event)),
                id: _ctx.id,
                onShow: _ctx.onShow,
                onHide: _ctx.onHide,
                maximized: _ctx.$q.platform.is.mobile
              }, {
                default: _withCtx(() => [
                  _createVNode(_component_q_card, {
                    ref: "dialogCard",
                    flat: "",
                    bordered: "",
                    style: _normalizeStyle({width:((_ctx.width||760)+'px'),'max-width':(_ctx.$q.platform.is.mobile?'100vw':'90vw')})
                  }, {
                    default: _withCtx(() => [
                      _createVNode(_component_q_card_actions, {
                        ref: "dialogHeader",
                        style: _normalizeStyle({cursor:(_ctx.draggable?'move':'default')})
                      }, {
                        default: _withCtx(() => [
                          _createElementVNode("h5", { class: "q-pl-sm non-selectable" }, _toDisplayString(_ctx.title), 1),
                          _createVNode(_component_q_space),
                          _withDirectives(_createVNode(_component_q_btn, {
                            icon: "close",
                            flat: "",
                            round: "",
                            dense: ""
                          }, null, 512), [
                            [_directive_close_popup]
                          ])
                        ]),
                        _: 1
                      }, 8, ["style"]),
                      _createVNode(_component_q_separator),
                      _createVNode(_component_q_card_section, { ref: "dialogBody" }, {
                        default: _withCtx(() => [
                          _renderSlot(_ctx.$slots, "default")
                        ]),
                        _: 3
                      }, 512)
                    ]),
                    _: 3
                  }, 8, ["style"])
                ]),
                _: 3
              }, 8, ["model-value", "onUpdate:modelValue", "id", "onShow", "onHide", "maximized"]))
            }
        } },
        "m-display": { hash: "ccb7f263", make: function (Vue) {
            const { toDisplayString: _toDisplayString, createTextVNode: _createTextVNode, resolveComponent: _resolveComponent, withCtx: _withCtx, openBlock: _openBlock, createBlock: _createBlock, createCommentVNode: _createCommentVNode, renderSlot: _renderSlot, createElementBlock: _createElementBlock } = Vue

            return function render(_ctx, _cache) {
              const _component_q_tooltip = _resolveComponent("q-tooltip")
              const _component_q_input = _resolveComponent("q-input")

              return (_ctx.labelWrapper)
                ? (_openBlock(), _createBlock(_component_q_input, {
                    key: 0,
                    dense: "",
                    outlined: "",
                    readonly: "",
                    "stack-label": "",
                    autogrow: "",
                    "model-value": _ctx.displayValue,
                    label: _ctx.label,
                    for: _ctx.id,
                    name: _ctx.name,
                    loading: _ctx.loading
                  }, {
                    default: _withCtx(() => [
                      (_ctx.tooltip)
                        ? (_openBlock(), _createBlock(_component_q_tooltip, { key: 0 }, {
                            default: _withCtx(() => [
                              _createTextVNode(_toDisplayString(_ctx.tooltip), 1)
                            ]),
                            _: 1
                          }))
                        : _createCommentVNode("", true)
                    ]),
                    _: 1
                  }, 8, ["model-value", "label", "for", "name", "loading"]))
                : (_openBlock(), _createElementBlock("span", {
                    key: 1,
                    id: _ctx.id
                  }, [
                    (_ctx.tooltip)
                      ? (_openBlock(), _createBlock(_component_q_tooltip, { key: 0 }, {
                          default: _withCtx(() => [
                            _createTextVNode(_toDisplayString(_ctx.tooltip), 1)
                          ]),
                          _: 1
                        }))
                      : _createCommentVNode("", true),
                    _renderSlot(_ctx.$slots, "default"),
                    _createTextVNode(_toDisplayString(_ctx.displayValue), 1)
                  ], 8, ["id"]))
            }
        } },
        "m-drop-down": { hash: "8304979b", make: function (Vue) {
            const { toDisplayString: _toDisplayString, createTextVNode: _createTextVNode, resolveComponent: _resolveComponent, withCtx: _withCtx, openBlock: _openBlock, createBlock: _createBlock, createCommentVNode: _createCommentVNode, createVNode: _createVNode, renderList: _renderList, Fragment: _Fragment, createElementBlock: _createElementBlock, createElementVNode: _createElementVNode, renderSlot: _renderSlot, mergeProps: _mergeProps, createSlots: _createSlots } = Vue

            return function render(_ctx, _cache) {
              const _component_q_tooltip = _resolveComponent("q-tooltip")
              const _component_q_item_section = _resolveComponent("q-item-section")
              const _component_q_item = _resolveComponent("q-item")
              const _component_q_chip = _resolveComponent("q-chip")
              const _component_q_select = _resolveComponent("q-select")

              return (_openBlock(), _createBlock(_component_q_select, _mergeProps({
                ref: "qSelect",
                "model-value": _ctx.modelValue,
                "onUpdate:modelValue": _ctx.handleInput,
                dense: "",
                outlined: "",
                "options-dense": "",
                "use-input": "",
                "hide-selected": _ctx.multiple,
                name: _ctx.name,
                for: _ctx.id,
                form: _ctx.form,
                "input-debounce": _ctx.serverSearch ? _ctx.serverDelay : 100,
                onFilter: _ctx.filterFn,
                clearable: _ctx.allowEmpty||_ctx.multiple,
                disable: _ctx.disable,
                multiple: _ctx.multiple,
                "emit-value": !_ctx.onSelectGoTo,
                "map-options": "",
                behavior: "menu",
                "new-value-mode": _ctx.combo ? 'add-unique' : undefined,
                rules: [val => _ctx.allowEmpty||_ctx.multiple||_ctx.combo||val===''||(val&&val.length)||'Please select an option'],
                "stack-label": "",
                label: _ctx.label,
                loading: _ctx.loading,
                "bg-color": _ctx.bg,
                options: _ctx.curOptions
              }, _ctx.errorAttrs), _createSlots({
                "no-option": _withCtx(() => [
                  _createVNode(_component_q_item, null, {
                    default: _withCtx(() => [
                      _createVNode(_component_q_item_section, { class: "text-grey" }, {
                        default: _withCtx(() => [
                          _createTextVNode("No results")
                        ]),
                        _: 1
                      })
                    ]),
                    _: 1
                  })
                ]),
                after: _withCtx(() => [
                  _renderSlot(_ctx.$slots, "after")
                ]),
                default: _withCtx(() => [
                  (_ctx.tooltip)
                    ? (_openBlock(), _createBlock(_component_q_tooltip, { key: 0 }, {
                        default: _withCtx(() => [
                          _createTextVNode(_toDisplayString(_ctx.tooltip), 1)
                        ]),
                        _: 1
                      }))
                    : _createCommentVNode("", true)
                ]),
                _: 2
              }, [
                (_ctx.multiple)
                  ? {
                      name: "prepend",
                      fn: _withCtx(() => [
                        _createElementVNode("div", null, [
                          (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.modelValue, (valueEntry) => {
                            return (_openBlock(), _createBlock(_component_q_chip, {
                              key: valueEntry,
                              dense: "",
                              size: "md",
                              class: "q-my-xs",
                              removable: "",
                              onRemove: $event => (_ctx.removeValue(valueEntry))
                            }, {
                              default: _withCtx(() => [
                                _createTextVNode(_toDisplayString(_ctx.optionLabel(valueEntry)), 1)
                              ]),
                              _: 2
                            }, 1032, ["onRemove"]))
                          }), 128))
                        ])
                      ]),
                      key: "0"
                    }
                  : undefined
              ]), 1040, ["model-value", "onUpdate:modelValue", "hide-selected", "name", "for", "form", "input-debounce", "onFilter", "clearable", "disable", "multiple", "emit-value", "new-value-mode", "rules", "label", "loading", "bg-color", "options"]))
            }
        } },
        "m-dynamic-container": { hash: "a6e86558", make: function (Vue) {
            const { resolveDynamicComponent: _resolveDynamicComponent, openBlock: _openBlock, createBlock: _createBlock } = Vue

            return function render(_ctx, _cache) {
              return (_openBlock(), _createBlock(_resolveDynamicComponent(_ctx.curComponent)))
            }
        } },
        "m-dynamic-dialog": { hash: "8cccede7", make: function (Vue) {
            const { resolveComponent: _resolveComponent, normalizeClass: _normalizeClass, createVNode: _createVNode, resolveDynamicComponent: _resolveDynamicComponent, openBlock: _openBlock, createBlock: _createBlock, withCtx: _withCtx, createElementBlock: _createElementBlock } = Vue

            return function render(_ctx, _cache) {
              const _component_q_btn = _resolveComponent("q-btn")
              const _component_m_dialog = _resolveComponent("m-dialog")

              return (_openBlock(), _createElementBlock("span", null, [
                _createVNode(_component_q_btn, {
                  dense: "",
                  outline: "",
                  "no-caps": "",
                  icon: "open_in_new",
                  label: _ctx.buttonText,
                  color: _ctx.color,
                  class: _normalizeClass(_ctx.buttonClass),
                  onClick: $event => (_ctx.isShown = true)
                }, null, 8, ["label", "color", "class", "onClick"]),
                _createVNode(_component_m_dialog, {
                  ref: "dialog",
                  modelValue: _ctx.isShown,
                  "onUpdate:modelValue": $event => ((_ctx.isShown) = $event),
                  id: _ctx.id,
                  title: _ctx.title,
                  color: _ctx.color,
                  width: _ctx.width
                }, {
                  default: _withCtx(() => [
                    (_openBlock(), _createBlock(_resolveDynamicComponent(_ctx.curComponent)))
                  ]),
                  _: 1
                }, 8, ["modelValue", "onUpdate:modelValue", "id", "title", "color", "width"])
              ]))
            }
        } },
        "m-editable": { hash: "83a53495", make: function (Vue) {
            const { createElementVNode: _createElementVNode, resolveComponent: _resolveComponent, withKeys: _withKeys, createVNode: _createVNode, withCtx: _withCtx, resolveDynamicComponent: _resolveDynamicComponent, openBlock: _openBlock, createBlock: _createBlock } = Vue

            return function render(_ctx, _cache) {
              const _component_q_input = _resolveComponent("q-input")
              const _component_q_popup_edit = _resolveComponent("q-popup-edit")

              return (_openBlock(), _createBlock(_resolveDynamicComponent(_ctx.labelType), {
                id: _ctx.id,
                class: "editable-label cursor-pointer",
                title: _ctx.tooltip
              }, {
                default: _withCtx(() => [
                  _createElementVNode("span", { innerHTML: _ctx.shown }, null, 8, ["innerHTML"]),
                  _createVNode(_component_q_popup_edit, {
                    modelValue: _ctx.editValue,
                    "onUpdate:modelValue": $event => ((_ctx.editValue) = $event),
                    buttons: "",
                    "label-set": _ctx.submit,
                    "label-cancel": _ctx.cancel,
                    onBeforeShow: _ctx.beforeShow,
                    onSave: _ctx.save
                  }, {
                    default: _withCtx((scope) => [
                      _createVNode(_component_q_input, {
                        modelValue: scope.value,
                        "onUpdate:modelValue": $event => ((scope.value) = $event),
                        dense: "",
                        autofocus: "",
                        type: _ctx.widgetType === 'textarea' ? 'textarea' : 'text',
                        loading: _ctx.saving,
                        onKeyup: _withKeys($event => (_ctx.widgetType === 'textarea' ? null : scope.set()), ["enter"])
                      }, null, 8, ["modelValue", "onUpdate:modelValue", "type", "loading", "onKeyup"])
                    ]),
                    _: 1
                  }, 8, ["modelValue", "onUpdate:modelValue", "label-set", "label-cancel", "onBeforeShow", "onSave"])
                ]),
                _: 1
              }, 8, ["id", "title"]))
            }
        } },
        "m-field": { hash: "2dc33bf6", make: function (Vue) {
            const { renderSlot: _renderSlot, resolveComponent: _resolveComponent, mergeProps: _mergeProps, withCtx: _withCtx, openBlock: _openBlock, createBlock: _createBlock } = Vue

            return function render(_ctx, _cache) {
              const _component_q_field = _resolveComponent("q-field")

              return (_openBlock(), _createBlock(_component_q_field, _mergeProps({
                dense: "",
                outlined: "",
                "stack-label": "",
                label: _ctx.label,
                "bg-color": _ctx.bgColor,
                disable: _ctx.disable
              }, _ctx.$attrs, { "model-value": 1 }), {
                control: _withCtx(() => [
                  _renderSlot(_ctx.$slots, "default")
                ]),
                _: 3
              }, 16, ["label", "bg-color", "disable"]))
            }
        } },
        "m-file": { hash: "91fcaf60", make: function (Vue) {
            const { toDisplayString: _toDisplayString, createTextVNode: _createTextVNode, resolveComponent: _resolveComponent, withCtx: _withCtx, openBlock: _openBlock, createBlock: _createBlock, createCommentVNode: _createCommentVNode, mergeProps: _mergeProps } = Vue

            return function render(_ctx, _cache) {
              const _component_q_tooltip = _resolveComponent("q-tooltip")
              const _component_q_file = _resolveComponent("q-file")

              return (_openBlock(), _createBlock(_component_q_file, _mergeProps({
                dense: "",
                outlined: "",
                "stack-label": "",
                label: _ctx.label,
                "model-value": _ctx.modelValue,
                "onUpdate:modelValue": $event => (_ctx.$emit('update:modelValue', $event)),
                name: _ctx.name,
                for: _ctx.id,
                size: _ctx.size,
                multiple: _ctx.multiple,
                accept: _ctx.accept,
                form: _ctx.form,
                disable: _ctx.disable,
                "bg-color": _ctx.bg
              }, _ctx.errorAttrs), {
                default: _withCtx(() => [
                  (_ctx.tooltip)
                    ? (_openBlock(), _createBlock(_component_q_tooltip, { key: 0 }, {
                        default: _withCtx(() => [
                          _createTextVNode(_toDisplayString(_ctx.tooltip), 1)
                        ]),
                        _: 1
                      }))
                    : _createCommentVNode("", true)
                ]),
                _: 1
              }, 16, ["label", "model-value", "onUpdate:modelValue", "name", "for", "size", "multiple", "accept", "form", "disable", "bg-color"]))
            }
        } },
        "m-form": { hash: "ffcdd952", make: function (Vue) {
            const { renderSlot: _renderSlot, resolveComponent: _resolveComponent, withModifiers: _withModifiers, withCtx: _withCtx, openBlock: _openBlock, createBlock: _createBlock } = Vue

            return function render(_ctx, _cache) {
              const _component_q_form = _resolveComponent("q-form")

              return (_openBlock(), _createBlock(_component_q_form, {
                ref: "qForm",
                onSubmit: _withModifiers(_ctx.submitForm, ["prevent"]),
                onReset: _withModifiers(_ctx.resetForm, ["prevent"]),
                autocapitalize: "off",
                autocomplete: "off"
              }, {
                default: _withCtx(() => [
                  _renderSlot(_ctx.$slots, "default", {
                    fields: _ctx.fields,
                    checkboxAllState: _ctx.checkboxAllState,
                    setCheckboxAllState: _ctx.setCheckboxAllState,
                    checkboxStates: _ctx.checkboxStates,
                    clickCheckbox: _ctx.clickCheckbox,
                    addCheckboxParameters: _ctx.addCheckboxParameters,
                    blurSubmitForm: _ctx.blurSubmitForm,
                    hasFieldsChanged: _ctx.hasFieldsChanged,
                    fieldChanged: _ctx.fieldChanged
                  })
                ]),
                _: 3
              }, 8, ["onSubmit", "onReset"]))
            }
        } },
        "m-form-column-config": { hash: "ab49c579", make: function (Vue) {
            const { renderList: _renderList, Fragment: _Fragment, openBlock: _openBlock, createElementBlock: _createElementBlock, toDisplayString: _toDisplayString, createTextVNode: _createTextVNode, resolveComponent: _resolveComponent, withCtx: _withCtx, createVNode: _createVNode, createBlock: _createBlock, createCommentVNode: _createCommentVNode, resolveDirective: _resolveDirective, withDirectives: _withDirectives, withModifiers: _withModifiers, createElementVNode: _createElementVNode } = Vue

            return function render(_ctx, _cache) {
              const _component_q_item_label = _resolveComponent("q-item-label")
              const _component_q_tooltip = _resolveComponent("q-tooltip")
              const _component_q_btn = _resolveComponent("q-btn")
              const _component_q_item_section = _resolveComponent("q-item-section")
              const _component_q_item = _resolveComponent("q-item")
              const _component_q_list = _resolveComponent("q-list")
              const _component_q_btn_dropdown = _resolveComponent("q-btn-dropdown")
              const _component_q_btn_group = _resolveComponent("q-btn-group")
              const _component_m_form = _resolveComponent("m-form")
              const _directive_close_popup = _resolveDirective("close-popup")

              return (_openBlock(), _createBlock(_component_m_form, {
                ref: "mForm",
                id: _ctx.id,
                action: _ctx.action
              }, {
                default: _withCtx(() => [
                  (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.columns, (column, columnIdx) => {
                    return (_openBlock(), _createBlock(_component_q_list, {
                      key: column.id,
                      bordered: "",
                      dense: ""
                    }, {
                      default: _withCtx(() => [
                        _createVNode(_component_q_item_label, { header: "" }, {
                          default: _withCtx(() => [
                            _createTextVNode(_toDisplayString(column.label), 1)
                          ]),
                          _: 2
                        }, 1024),
                        (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(column.children, (field, fieldIdx) => {
                          return (_openBlock(), _createBlock(_component_q_item, {
                            key: field.id
                          }, {
                            default: _withCtx(() => [
                              (columnIdx !== 0)
                                ? (_openBlock(), _createBlock(_component_q_item_section, {
                                    key: 0,
                                    side: ""
                                  }, {
                                    default: _withCtx(() => [
                                      _createVNode(_component_q_btn, {
                                        dense: "",
                                        flat: "",
                                        icon: "cancel",
                                        onClick: $event => (_ctx.hideField(columnIdx, fieldIdx))
                                      }, {
                                        default: _withCtx(() => [
                                          _createVNode(_component_q_tooltip, null, {
                                            default: _withCtx(() => [
                                              _createTextVNode("Hide")
                                            ]),
                                            _: 1
                                          })
                                        ]),
                                        _: 1
                                      }, 8, ["onClick"])
                                    ]),
                                    _: 2
                                  }, 1024))
                                : _createCommentVNode("", true),
                              _createVNode(_component_q_item_section, null, {
                                default: _withCtx(() => [
                                  _createVNode(_component_q_item_label, null, {
                                    default: _withCtx(() => [
                                      _createTextVNode(_toDisplayString(field.label), 1)
                                    ]),
                                    _: 2
                                  }, 1024)
                                ]),
                                _: 2
                              }, 1024),
                              (columnIdx === 0)
                                ? (_openBlock(), _createBlock(_component_q_item_section, {
                                    key: 1,
                                    side: ""
                                  }, {
                                    default: _withCtx(() => [
                                      _createVNode(_component_q_btn_dropdown, {
                                        dense: "",
                                        outline: "",
                                        "no-caps": "",
                                        label: "Display"
                                      }, {
                                        default: _withCtx(() => [
                                          _createVNode(_component_q_list, { dense: "" }, {
                                            default: _withCtx(() => [
                                              (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.columns.slice(1), (toColumn, toColumnIdx) => {
                                                return _withDirectives((_openBlock(), _createBlock(_component_q_item, {
                                                  key: toColumn.id,
                                                  clickable: ""
                                                }, {
                                                  default: _withCtx(() => [
                                                    _createVNode(_component_q_item_section, {
                                                      onClick: $event => (_ctx.moveToCol(columnIdx, fieldIdx, toColumnIdx+1))
                                                    }, {
                                                      default: _withCtx(() => [
                                                        _createTextVNode(_toDisplayString(toColumn.label), 1)
                                                      ]),
                                                      _: 2
                                                    }, 1032, ["onClick"])
                                                  ]),
                                                  _: 2
                                                }, 1024)), [
                                                  [_directive_close_popup]
                                                ])
                                              }), 128)),
                                              _withDirectives((_openBlock(), _createBlock(_component_q_item, { clickable: "" }, {
                                                default: _withCtx(() => [
                                                  _createVNode(_component_q_item_section, {
                                                    onClick: $event => (_ctx.moveToCol(columnIdx, fieldIdx, _ctx.columns.length+1))
                                                  }, {
                                                    default: _withCtx(() => [
                                                      _createTextVNode("New Column")
                                                    ]),
                                                    _: 1
                                                  }, 8, ["onClick"])
                                                ]),
                                                _: 2
                                              }, 1024)), [
                                                [_directive_close_popup]
                                              ])
                                            ]),
                                            _: 2
                                          }, 1024)
                                        ]),
                                        _: 2
                                      }, 1024)
                                    ]),
                                    _: 2
                                  }, 1024))
                                : (_openBlock(), _createBlock(_component_q_item_section, {
                                    key: 2,
                                    side: ""
                                  }, {
                                    default: _withCtx(() => [
                                      _createVNode(_component_q_btn_group, { flat: "" }, {
                                        default: _withCtx(() => [
                                          _createVNode(_component_q_btn, {
                                            disabled: columnIdx <= 1,
                                            dense: "",
                                            flat: "",
                                            icon: "north",
                                            onClick: $event => (_ctx.moveToCol(columnIdx, fieldIdx, columnIdx-1))
                                          }, null, 8, ["disabled", "onClick"]),
                                          _createVNode(_component_q_btn, {
                                            disabled: fieldIdx === 0,
                                            dense: "",
                                            flat: "",
                                            icon: "expand_less",
                                            onClick: $event => (_ctx.moveInCol(columnIdx, fieldIdx, fieldIdx-1))
                                          }, null, 8, ["disabled", "onClick"]),
                                          _createVNode(_component_q_btn, {
                                            disabled: (fieldIdx + 1) === column.children.length,
                                            dense: "",
                                            flat: "",
                                            icon: "expand_more",
                                            onClick: $event => (_ctx.moveInCol(columnIdx, fieldIdx, fieldIdx+1))
                                          }, null, 8, ["disabled", "onClick"]),
                                          _createVNode(_component_q_btn, {
                                            dense: "",
                                            flat: "",
                                            icon: "south",
                                            onClick: $event => (_ctx.moveToCol(columnIdx, fieldIdx, columnIdx+1))
                                          }, null, 8, ["onClick"])
                                        ]),
                                        _: 2
                                      }, 1024)
                                    ]),
                                    _: 2
                                  }, 1024))
                            ]),
                            _: 2
                          }, 1024))
                        }), 128))
                      ]),
                      _: 2
                    }, 1024))
                  }), 128)),
                  _createElementVNode("div", { class: "q-my-md" }, [
                    _createVNode(_component_q_btn, {
                      dense: "",
                      outline: "",
                      "no-caps": "",
                      onClick: _withModifiers($event => (_ctx.saveColumns()), ["prevent"]),
                      label: "Save Changes"
                    }, null, 8, ["onClick"]),
                    _createVNode(_component_q_btn, {
                      dense: "",
                      outline: "",
                      "no-caps": "",
                      onClick: _withModifiers($event => (_ctx.resetColumns()), ["prevent"]),
                      label: "Undo Changes"
                    }, null, 8, ["onClick"]),
                    _createVNode(_component_q_btn, {
                      dense: "",
                      outline: "",
                      "no-caps": "",
                      onClick: _withModifiers($event => (_ctx.resetToDefault()), ["prevent"]),
                      label: "Reset to Default"
                    }, null, 8, ["onClick"])
                  ])
                ]),
                _: 1
              }, 8, ["id", "action"]))
            }
        } },
        "m-form-go-page": { hash: "9cf1e73b", make: function (Vue) {
            const { resolveComponent: _resolveComponent, createVNode: _createVNode, withCtx: _withCtx, withModifiers: _withModifiers, openBlock: _openBlock, createBlock: _createBlock, createCommentVNode: _createCommentVNode } = Vue

            return function render(_ctx, _cache) {
              const _component_q_btn = _resolveComponent("q-btn")
              const _component_q_input = _resolveComponent("q-input")
              const _component_q_form = _resolveComponent("q-form")

              return (!_ctx.formList || (_ctx.formList.paginate && _ctx.formList.paginate.pageMaxIndex > 4))
                ? (_openBlock(), _createBlock(_component_q_form, {
                    key: 0,
                    onSubmit: _withModifiers(_ctx.goPage, ["prevent"])
                  }, {
                    default: _withCtx(() => [
                      _createVNode(_component_q_input, {
                        dense: "",
                        outlined: "",
                        modelValue: _ctx.pageIndex,
                        "onUpdate:modelValue": $event => ((_ctx.pageIndex) = $event),
                        type: "text",
                        size: "4",
                        name: "pageIndex",
                        placeholder: _ctx.$q.lang.label ? 'Page #' : 'Page #',
                        rules: [val => /^\d*$/.test(val) || 'digits only', val => !val || ((_ctx.formList && +val <= _ctx.formList.paginate.pageMaxIndex + 1) || (_ctx.maxIndex && +val <= _ctx.maxIndex + 1)) || 'higher than max']
                      }, {
                        append: _withCtx(() => [
                          _createVNode(_component_q_btn, {
                            dense: "",
                            flat: "",
                            "no-caps": "",
                            type: "submit",
                            icon: "redo"
                          })
                        ]),
                        _: 1
                      }, 8, ["modelValue", "onUpdate:modelValue", "placeholder", "rules"])
                    ]),
                    _: 1
                  }, 8, ["onSubmit"]))
                : _createCommentVNode("", true)
            }
        } },
        "m-form-link": { hash: "815e6a35", make: function (Vue) {
            const { renderSlot: _renderSlot, resolveComponent: _resolveComponent, withModifiers: _withModifiers, withCtx: _withCtx, openBlock: _openBlock, createBlock: _createBlock } = Vue

            return function render(_ctx, _cache) {
              const _component_q_form = _resolveComponent("q-form")

              return (_openBlock(), _createBlock(_component_q_form, {
                ref: "qForm",
                onSubmit: _withModifiers(_ctx.submitForm, ["prevent"]),
                onReset: _withModifiers(_ctx.resetForm, ["prevent"]),
                autocapitalize: "off",
                autocomplete: "off"
              }, {
                default: _withCtx(() => [
                  _renderSlot(_ctx.$slots, "default", {
                    clearForm: _ctx.clearForm,
                    fields: _ctx.fields,
                    hasFieldsChanged: _ctx.hasFieldsChanged,
                    fieldChanged: _ctx.fieldChanged
                  })
                ]),
                _: 3
              }, 8, ["onSubmit", "onReset"]))
            }
        } },
        "m-form-list": { hash: "60e77322", make: function (Vue) {
            const { renderList: _renderList, Fragment: _Fragment, openBlock: _openBlock, createElementBlock: _createElementBlock, renderSlot: _renderSlot, resolveComponent: _resolveComponent, withCtx: _withCtx, createBlock: _createBlock, createCommentVNode: _createCommentVNode, createElementVNode: _createElementVNode, createVNode: _createVNode, normalizeClass: _normalizeClass } = Vue

            return function render(_ctx, _cache) {
              const _component_m_form = _resolveComponent("m-form")
              const _component_q_space = _resolveComponent("q-space")
              const _component_m_form_paginate = _resolveComponent("m-form-paginate")
              const _component_m_form_go_page = _resolveComponent("m-form-go-page")
              const _component_q_btn = _resolveComponent("q-btn")
              const _component_m_form_link = _resolveComponent("m-form-link")
              const _component_q_checkbox = _resolveComponent("q-checkbox")

              return (_openBlock(), _createElementBlock("div", null, [
                (!_ctx.multi && !_ctx.skipForm)
                  ? (_openBlock(true), _createElementBlock(_Fragment, { key: 0 }, _renderList(_ctx.rowList, (fields, rowIndex) => {
                      return (_openBlock(), _createBlock(_component_m_form, {
                        key: rowIndex,
                        name: _ctx.idVal+'_'+rowIndex,
                        id: _ctx.idVal+'_'+rowIndex,
                        action: _ctx.action,
                        "fields-initial": fields
                      }, {
                        default: _withCtx(() => [
                          _renderSlot(_ctx.$slots, "rowForm", {
                            fields: fields,
                            rowIndex: rowIndex
                          })
                        ]),
                        _: 2
                      }, 1032, ["name", "id", "action", "fields-initial"]))
                    }), 128))
                  : _createCommentVNode("", true),
                (_ctx.multi && !_ctx.skipForm)
                  ? (_openBlock(), _createBlock(_component_m_form, {
                      key: 1,
                      name: _ctx.idVal,
                      id: _ctx.idVal,
                      action: _ctx.action,
                      "fields-initial": _ctx.multiFields
                    }, {
                      default: _withCtx(() => [
                        _createElementVNode("input", {
                          type: "hidden",
                          name: "moquiFormName",
                          value: _ctx.name
                        }, null, 8, ["value"]),
                        _createElementVNode("input", {
                          type: "hidden",
                          name: "_isMulti",
                          value: "true"
                        }),
                        (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.rowList, (fields, rowIndex) => {
                          return _renderSlot(_ctx.$slots, "rowForm", {
                            fields: fields,
                            rowIndex: rowIndex
                          }, undefined, undefined, rowIndex)
                        }), 128))
                      ]),
                      _: 3
                    }, 8, ["name", "id", "action", "fields-initial"]))
                  : _createCommentVNode("", true),
                _createElementVNode("div", {
                  class: _normalizeClass(["q-my-sm q-table__container q-table__card q-table--horizontal-separator q-table--dense q-table--flat", {'q-table--dark':_ctx.$q.dark.isActive}])
                }, [
                  _createElementVNode("div", { class: "form-list-nav-row row items-center" }, [
                    _renderSlot(_ctx.$slots, "navStart"),
                    _createVNode(_component_q_space),
                    _createVNode(_component_m_form_paginate, {
                      paginate: _ctx.paginate,
                      "form-list": this
                    }, null, 8, ["paginate"]),
                    _createVNode(_component_m_form_go_page, {
                      "id-val": _ctx.idVal,
                      "form-list": this
                    }, null, 8, ["id-val"]),
                    (_ctx.csvButton)
                      ? (_openBlock(), _createBlock(_component_q_btn, {
                          key: 0,
                          dense: "",
                          outline: "",
                          type: "a",
                          href: _ctx.csvUrl,
                          label: "CSV"
                        }, null, 8, ["href"]))
                      : _createCommentVNode("", true),
                    _renderSlot(_ctx.$slots, "nav")
                  ]),
                  _createElementVNode("div", {
                    class: "table q-table",
                    id: _ctx.idVal+'_table'
                  }, [
                    (!_ctx.skipHeader)
                      ? (_openBlock(), _createElementBlock("div", {
                          key: 0,
                          class: "thead"
                        }, [
                          (_ctx.headerForm && !_ctx.headerDialog)
                            ? (_openBlock(), _createBlock(_component_m_form_link, {
                                key: 0,
                                name: _ctx.idVal+'_header',
                                id: _ctx.idVal+'_header',
                                action: _ctx.$root.currentLinkPath,
                                "fields-initial": _ctx.searchObj
                              }, {
                                default: _withCtx((formProps) => [
                                  _renderSlot(_ctx.$slots, "headerForm", {
                                    search: formProps.fields
                                  })
                                ]),
                                _: 3
                              }, 8, ["name", "id", "action", "fields-initial"]))
                            : _createCommentVNode("", true),
                          _createElementVNode("div", { class: "tr" }, [
                            (_ctx.selection)
                              ? (_openBlock(), _createElementBlock("div", {
                                  key: 0,
                                  class: "th"
                                }, [
                                  _createVNode(_component_q_checkbox, {
                                    size: "sm",
                                    "model-value": _ctx.allSelected,
                                    "onUpdate:modelValue": _ctx.selectAll
                                  }, null, 8, ["model-value", "onUpdate:modelValue"])
                                ]))
                              : _createCommentVNode("", true),
                            _renderSlot(_ctx.$slots, "header", {
                              search: _ctx.searchObj,
                              sortBy: _ctx.sortBy,
                              orderBy: _ctx.orderBy
                            })
                          ])
                        ]))
                      : _createCommentVNode("", true),
                    _createElementVNode("div", { class: "tbody" }, [
                      (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.rowList, (fields, rowIndex) => {
                        return (_openBlock(), _createElementBlock(_Fragment, { key: rowIndex }, [
                          _createElementVNode("div", {
                            class: _normalizeClass(["tr", {selected:_ctx.selected[rowIndex]}])
                          }, [
                            (_ctx.selection)
                              ? (_openBlock(), _createElementBlock("div", {
                                  key: 0,
                                  class: "td"
                                }, [
                                  _createVNode(_component_q_checkbox, {
                                    size: "sm",
                                    "model-value": !!_ctx.selected[rowIndex],
                                    "onUpdate:modelValue": $event => (_ctx.selected[rowIndex] = $event)
                                  }, null, 8, ["model-value", "onUpdate:modelValue"])
                                ]))
                              : _createCommentVNode("", true),
                            _renderSlot(_ctx.$slots, "row", {
                              fields: fields,
                              rowIndex: rowIndex,
                              selected: !!_ctx.selected[rowIndex]
                            })
                          ], 2),
                          (fields.aggregateSubList && fields.aggregateSubList.length)
                            ? (_openBlock(true), _createElementBlock(_Fragment, { key: 0 }, _renderList(fields.aggregateSubList, (subRow, subIndex) => {
                                return (_openBlock(), _createElementBlock("div", {
                                  key: rowIndex+'_'+subIndex,
                                  class: "tr form-list-sub-row"
                                }, [
                                  _renderSlot(_ctx.$slots, "subRow", {
                                    row: fields,
                                    subRow: subRow,
                                    subIndex: subIndex
                                  })
                                ]))
                              }), 128))
                            : _createCommentVNode("", true)
                        ], 64))
                      }), 128)),
                      (!_ctx.rowList.length && !_ctx.loading)
                        ? (_openBlock(), _createElementBlock("div", {
                            key: 0,
                            class: "tr"
                          }, [
                            _createElementVNode("div", { class: "td" }, "No results found")
                          ]))
                        : _createCommentVNode("", true)
                    ])
                  ], 8, ["id"])
                ], 2)
              ]))
            }
        } },
        "m-form-paginate": { hash: "7552bc4a", make: function (Vue) {
            const { resolveComponent: _resolveComponent, withModifiers: _withModifiers, createVNode: _createVNode, Fragment: _Fragment, openBlock: _openBlock, createElementBlock: _createElementBlock, createCommentVNode: _createCommentVNode, renderList: _renderList, createBlock: _createBlock, toDisplayString: _toDisplayString, createTextVNode: _createTextVNode, withCtx: _withCtx } = Vue

            return function render(_ctx, _cache) {
              const _component_q_btn = _resolveComponent("q-btn")

              return (_ctx.paginate && _ctx.paginate.count > 1)
                ? (_openBlock(), _createElementBlock("div", {
                    key: 0,
                    class: "q-pagination row no-wrap items-center"
                  }, [
                    (_ctx.paginate.pageIndex > 0)
                      ? (_openBlock(), _createElementBlock(_Fragment, { key: 0 }, [
                          _createVNode(_component_q_btn, {
                            dense: "",
                            flat: "",
                            "no-caps": "",
                            onClick: _withModifiers($event => (_ctx.setIndex(0)), ["prevent"]),
                            icon: "skip_previous"
                          }, null, 8, ["onClick"]),
                          _createVNode(_component_q_btn, {
                            dense: "",
                            flat: "",
                            "no-caps": "",
                            onClick: _withModifiers($event => (_ctx.setIndex(_ctx.paginate.pageIndex-1)), ["prevent"]),
                            icon: "fast_rewind"
                          }, null, 8, ["onClick"])
                        ], 64))
                      : (_openBlock(), _createElementBlock(_Fragment, { key: 1 }, [
                          _createVNode(_component_q_btn, {
                            dense: "",
                            flat: "",
                            "no-caps": "",
                            disabled: "",
                            icon: "skip_previous"
                          }),
                          _createVNode(_component_q_btn, {
                            dense: "",
                            flat: "",
                            "no-caps": "",
                            disabled: "",
                            icon: "fast_rewind"
                          })
                        ], 64)),
                    (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.prevArray, (prevIndex) => {
                      return (_openBlock(), _createBlock(_component_q_btn, {
                        key: prevIndex,
                        dense: "",
                        flat: "",
                        "no-caps": "",
                        onClick: _withModifiers($event => (_ctx.setIndex(prevIndex)), ["prevent"]),
                        label: prevIndex+1,
                        color: "primary"
                      }, null, 8, ["onClick", "label"]))
                    }), 128)),
                    _createVNode(_component_q_btn, {
                      dense: "",
                      flat: "",
                      "no-caps": "",
                      disabled: ""
                    }, {
                      default: _withCtx(() => [
                        _createTextVNode(_toDisplayString(_ctx.paginate.pageIndex+1) + " / " + _toDisplayString(_ctx.paginate.pageMaxIndex+1) + " (" + _toDisplayString(_ctx.paginate.pageRangeLow) + "-" + _toDisplayString(_ctx.paginate.pageRangeHigh) + " / " + _toDisplayString(_ctx.paginate.count) + ")", 1)
                      ]),
                      _: 1
                    }),
                    (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.nextArray, (nextIndex) => {
                      return (_openBlock(), _createBlock(_component_q_btn, {
                        key: nextIndex,
                        dense: "",
                        flat: "",
                        "no-caps": "",
                        onClick: _withModifiers($event => (_ctx.setIndex(nextIndex)), ["prevent"]),
                        label: nextIndex+1,
                        color: "primary"
                      }, null, 8, ["onClick", "label"]))
                    }), 128)),
                    (_ctx.paginate.pageIndex < _ctx.paginate.pageMaxIndex)
                      ? (_openBlock(), _createElementBlock(_Fragment, { key: 2 }, [
                          _createVNode(_component_q_btn, {
                            dense: "",
                            flat: "",
                            "no-caps": "",
                            onClick: _withModifiers($event => (_ctx.setIndex(_ctx.paginate.pageIndex+1)), ["prevent"]),
                            icon: "fast_forward"
                          }, null, 8, ["onClick"]),
                          _createVNode(_component_q_btn, {
                            dense: "",
                            flat: "",
                            "no-caps": "",
                            onClick: _withModifiers($event => (_ctx.setIndex(_ctx.paginate.pageMaxIndex)), ["prevent"]),
                            icon: "skip_next"
                          }, null, 8, ["onClick"])
                        ], 64))
                      : (_openBlock(), _createElementBlock(_Fragment, { key: 3 }, [
                          _createVNode(_component_q_btn, {
                            dense: "",
                            flat: "",
                            "no-caps": "",
                            disabled: "",
                            icon: "fast_forward"
                          }),
                          _createVNode(_component_q_btn, {
                            dense: "",
                            flat: "",
                            "no-caps": "",
                            disabled: "",
                            icon: "skip_next"
                          })
                        ], 64))
                  ]))
                : _createCommentVNode("", true)
            }
        } },
        "m-link": { hash: "5b271294", make: function (Vue) {
            const { renderSlot: _renderSlot, withModifiers: _withModifiers, openBlock: _openBlock, createElementBlock: _createElementBlock } = Vue

            return function render(_ctx, _cache) {
              return (_openBlock(), _createElementBlock("a", {
                href: _ctx.linkHref,
                onClick: _withModifiers(_ctx.go, ["prevent"]),
                class: "q-link"
              }, [
                _renderSlot(_ctx.$slots, "default")
              ], 8, ["href", "onClick"]))
            }
        } },
        "m-menu-item-content": { hash: "87f2da77", make: function (Vue) {
            const { normalizeClass: _normalizeClass, openBlock: _openBlock, createElementBlock: _createElementBlock, createCommentVNode: _createCommentVNode, createElementVNode: _createElementVNode, toDisplayString: _toDisplayString } = Vue

            return function render(_ctx, _cache) {
              return (_openBlock(), _createElementBlock("div", { class: "q-item__section column q-item__section--main justify-center" }, [
                _createElementVNode("div", { class: "q-item__label" }, [
                  (_ctx.menuItem.image && _ctx.menuItem.imageType === 'icon')
                    ? (_openBlock(), _createElementBlock("i", {
                        key: 0,
                        class: _normalizeClass(_ctx.menuItem.image),
                        style: {"padding-right":"8px"}
                      }, null, 2))
                    : (_ctx.menuItem.image)
                      ? (_openBlock(), _createElementBlock("span", {
                          key: 1,
                          style: {"padding-right":"8px"}
                        }, [
                          _createElementVNode("img", {
                            src: _ctx.menuItem.image,
                            alt: _ctx.menuItem.title,
                            height: "14",
                            class: "invertible"
                          }, null, 8, ["src", "alt"])
                        ]))
                      : _createCommentVNode("", true),
                  _createElementVNode("span", {
                    class: _normalizeClass({'text-primary':_ctx.active})
                  }, _toDisplayString(_ctx.menuItem.title), 3)
                ])
              ]))
            }
        } },
        "m-menu-nav-item": { hash: "72768a09", make: function (Vue) {
            const { resolveComponent: _resolveComponent, createVNode: _createVNode, renderList: _renderList, Fragment: _Fragment, openBlock: _openBlock, createElementBlock: _createElementBlock, createBlock: _createBlock, withCtx: _withCtx, createCommentVNode: _createCommentVNode } = Vue

            return function render(_ctx, _cache) {
              const _component_m_menu_item_content = _resolveComponent("m-menu-item-content")
              const _component_m_menu_subscreen_item = _resolveComponent("m-menu-subscreen-item")
              const _component_q_expansion_item = _resolveComponent("q-expansion-item")
              const _component_m_menu_nav_item = _resolveComponent("m-menu-nav-item")

              return (_ctx.navMenuItem && _ctx.navMenuItem.subscreens && _ctx.navMenuItem.subscreens.length)
                ? (_openBlock(), _createBlock(_component_q_expansion_item, {
                    key: 0,
                    "model-value": true,
                    "content-inset-level": 0.3,
                    "switch-toggle-side": "",
                    dense: "",
                    "dense-toggle": "",
                    "expanded-icon": "arrow_drop_down",
                    to: _ctx.navMenuItem.pathWithParams
                  }, {
                    header: _withCtx(() => [
                      _createVNode(_component_m_menu_item_content, {
                        "menu-item": _ctx.navMenuItem,
                        active: ""
                      }, null, 8, ["menu-item"])
                    ]),
                    default: _withCtx(() => [
                      (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.navMenuItem.subscreens, (subscreen, ssIndex) => {
                        return (_openBlock(), _createBlock(_component_m_menu_subscreen_item, {
                          key: subscreen.name,
                          "menu-index": _ctx.menuIndex,
                          "subscreen-index": ssIndex
                        }, null, 8, ["menu-index", "subscreen-index"]))
                      }), 128))
                    ]),
                    _: 1
                  }, 8, ["to"]))
                : (_ctx.navMenuItem && _ctx.navMenuItem.savedFinds && _ctx.navMenuItem.savedFinds.length)
                  ? (_openBlock(), _createBlock(_component_q_expansion_item, {
                      key: 1,
                      "model-value": true,
                      "content-inset-level": 0.3,
                      "switch-toggle-side": "",
                      dense: "",
                      "dense-toggle": "",
                      "expanded-icon": "arrow_drop_down",
                      to: _ctx.navMenuItem.pathWithParams
                    }, {
                      header: _withCtx(() => [
                        _createVNode(_component_m_menu_item_content, {
                          "menu-item": _ctx.navMenuItem,
                          active: ""
                        }, null, 8, ["menu-item"])
                      ]),
                      default: _withCtx(() => [
                        (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.navMenuItem.savedFinds, (savedFind, ssIndex) => {
                          return (_openBlock(), _createBlock(_component_q_expansion_item, {
                            key: savedFind.name,
                            "model-value": false,
                            "switch-toggle-side": "",
                            dense: "",
                            "dense-toggle": "",
                            "expand-icon": "chevron_right",
                            to: savedFind.pathWithParams
                          }, {
                            header: _withCtx(() => [
                              _createVNode(_component_m_menu_item_content, {
                                "menu-item": savedFind,
                                active: savedFind.active
                              }, null, 8, ["menu-item", "active"])
                            ]),
                            _: 2
                          }, 1032, ["to"]))
                        }), 128))
                      ]),
                      _: 1
                    }, 8, ["to"]))
                  : (_ctx.navMenuItem && _ctx.menuIndex < (_ctx.navMenuLength - 1))
                    ? (_openBlock(), _createBlock(_component_q_expansion_item, {
                        key: 2,
                        "model-value": true,
                        "content-inset-level": 0.3,
                        "switch-toggle-side": "",
                        dense: "",
                        "dense-toggle": "",
                        "expanded-icon": "arrow_drop_down",
                        to: _ctx.navMenuItem.pathWithParams
                      }, {
                        header: _withCtx(() => [
                          _createVNode(_component_m_menu_item_content, {
                            "menu-item": _ctx.navMenuItem,
                            active: ""
                          }, null, 8, ["menu-item"])
                        ]),
                        default: _withCtx(() => [
                          _createVNode(_component_m_menu_nav_item, {
                            "menu-index": _ctx.menuIndex + 1
                          }, null, 8, ["menu-index"])
                        ]),
                        _: 1
                      }, 8, ["to"]))
                    : (_ctx.navMenuItem)
                      ? (_openBlock(), _createBlock(_component_q_expansion_item, {
                          key: 3,
                          "model-value": false,
                          "switch-toggle-side": "",
                          dense: "",
                          "dense-toggle": "",
                          "expand-icon": "arrow_right",
                          to: _ctx.navMenuItem.pathWithParams
                        }, {
                          header: _withCtx(() => [
                            _createVNode(_component_m_menu_item_content, {
                              "menu-item": _ctx.navMenuItem,
                              active: ""
                            }, null, 8, ["menu-item"])
                          ]),
                          _: 1
                        }, 8, ["to"]))
                      : _createCommentVNode("", true)
            }
        } },
        "m-menu-subscreen-item": { hash: "2e379235", make: function (Vue) {
            const { resolveComponent: _resolveComponent, openBlock: _openBlock, createBlock: _createBlock, createCommentVNode: _createCommentVNode, createVNode: _createVNode, withCtx: _withCtx } = Vue

            return function render(_ctx, _cache) {
              const _component_m_menu_nav_item = _resolveComponent("m-menu-nav-item")
              const _component_m_menu_item_content = _resolveComponent("m-menu-item-content")
              const _component_q_expansion_item = _resolveComponent("q-expansion-item")

              return (_ctx.subscreen.active)
                ? (_openBlock(), _createBlock(_component_m_menu_nav_item, {
                    key: 0,
                    "menu-index": _ctx.menuIndex + 1
                  }, null, 8, ["menu-index"]))
                : (_openBlock(), _createBlock(_component_q_expansion_item, {
                    key: 1,
                    "model-value": false,
                    "switch-toggle-side": "",
                    dense: "",
                    "dense-toggle": "",
                    "expand-icon": "arrow_right",
                    to: _ctx.subscreen.pathWithParams
                  }, {
                    header: _withCtx(() => [
                      _createVNode(_component_m_menu_item_content, { "menu-item": _ctx.subscreen }, null, 8, ["menu-item"])
                    ]),
                    _: 1
                  }, 8, ["to"]))
            }
        } },
        "m-mermaid": { hash: "981f6b40", make: function (Vue) {
            const { renderSlot: _renderSlot, createElementVNode: _createElementVNode, normalizeStyle: _normalizeStyle, openBlock: _openBlock, createElementBlock: _createElementBlock } = Vue

            return function render(_ctx, _cache) {
              return (_openBlock(), _createElementBlock("div", {
                class: "mermaid-container",
                style: _normalizeStyle({height:_ctx.height,width:_ctx.width})
              }, [
                _createElementVNode("pre", {
                  ref: "source",
                  style: {"display":"none"}
                }, [
                  _renderSlot(_ctx.$slots, "default")
                ], 512),
                _createElementVNode("div", { ref: "target" }, null, 512)
              ], 4))
            }
        } },
        "m-qapps-root": { hash: "d402adf4", make: function (Vue) {
            const { resolveComponent: _resolveComponent, createVNode: _createVNode, createElementVNode: _createElementVNode, withCtx: _withCtx, openBlock: _openBlock, createBlock: _createBlock, createCommentVNode: _createCommentVNode, toDisplayString: _toDisplayString, createTextVNode: _createTextVNode, renderList: _renderList, Fragment: _Fragment, createElementBlock: _createElementBlock, normalizeClass: _normalizeClass, resolveDirective: _resolveDirective, withDirectives: _withDirectives, resolveDynamicComponent: _resolveDynamicComponent, createSlots: _createSlots, withModifiers: _withModifiers, vShow: _vShow } = Vue

            return function render(_ctx, _cache) {
              const _component_q_btn = _resolveComponent("q-btn")
              const _component_m_link = _resolveComponent("m-link")
              const _component_q_toolbar_title = _resolveComponent("q-toolbar-title")
              const _component_q_item_section = _resolveComponent("q-item-section")
              const _component_q_item = _resolveComponent("q-item")
              const _component_q_list = _resolveComponent("q-list")
              const _component_q_menu = _resolveComponent("q-menu")
              const _component_q_icon = _resolveComponent("q-icon")
              const _component_q_space = _resolveComponent("q-space")
              const _component_q_circular_progress = _resolveComponent("q-circular-progress")
              const _component_q_tooltip = _resolveComponent("q-tooltip")
              const _component_m_dynamic_dialog = _resolveComponent("m-dynamic-dialog")
              const _component_q_banner = _resolveComponent("q-banner")
              const _component_q_card_section = _resolveComponent("q-card-section")
              const _component_q_separator = _resolveComponent("q-separator")
              const _component_q_card_actions = _resolveComponent("q-card-actions")
              const _component_q_card = _resolveComponent("q-card")
              const _component_q_toolbar = _resolveComponent("q-toolbar")
              const _component_q_header = _resolveComponent("q-header")
              const _component_m_menu_nav_item = _resolveComponent("m-menu-nav-item")
              const _component_m_subscreens_active = _resolveComponent("m-subscreens-active")
              const _component_q_page = _resolveComponent("q-page")
              const _component_q_page_container = _resolveComponent("q-page-container")
              const _component_q_footer = _resolveComponent("q-footer")
              const _component_q_layout = _resolveComponent("q-layout")
              const _component_m_text_line = _resolveComponent("m-text-line")
              const _component_q_form = _resolveComponent("q-form")
              const _component_m_dialog = _resolveComponent("m-dialog")
              const _directive_close_popup = _resolveDirective("close-popup")

              return (_openBlock(), _createElementBlock(_Fragment, null, [
                _createVNode(_component_q_layout, { view: "hHh LpR fFf" }, {
                  default: _withCtx(() => [
                    _createVNode(_component_q_header, {
                      reveal: "",
                      bordered: "",
                      class: "bg-black text-white",
                      id: "top"
                    }, {
                      default: _withCtx(() => [
                        _createVNode(_component_q_toolbar, { style: {"font-size":"15px"} }, {
                          default: _withCtx(() => [
                            _createVNode(_component_q_btn, {
                              dense: "",
                              flat: "",
                              icon: "menu",
                              onClick: $event => (_ctx.toggleLeftOpen())
                            }, null, 8, ["onClick"]),
                            (_ctx.shell.logoUrl)
                              ? (_openBlock(), _createBlock(_component_m_link, {
                                  key: 0,
                                  href: "/apps"
                                }, {
                                  default: _withCtx(() => [
                                    _createElementVNode("div", { class: "q-mx-md q-mt-sm" }, [
                                      _createElementVNode("img", {
                                        src: _ctx.shell.logoUrl,
                                        alt: "Home",
                                        height: "32"
                                      }, null, 8, ["src"])
                                    ])
                                  ]),
                                  _: 1
                                }))
                              : _createCommentVNode("", true),
                            (_ctx.shell.title)
                              ? (_openBlock(), _createBlock(_component_q_toolbar_title, { key: 1 }, {
                                  default: _withCtx(() => [
                                    _createTextVNode(_toDisplayString(_ctx.shell.title), 1)
                                  ]),
                                  _: 1
                                }))
                              : _createCommentVNode("", true),
                            (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.navMenuList, (navMenuItem, menuIndex) => {
                              return (_openBlock(), _createElementBlock(_Fragment, null, [
                                (menuIndex < (_ctx.navMenuList.length - 1))
                                  ? (_openBlock(), _createElementBlock(_Fragment, { key: 0 }, [
                                      (navMenuItem.hasTabMenu)
                                        ? (_openBlock(), _createBlock(_component_m_link, {
                                            key: 0,
                                            href: _ctx.getNavHref(menuIndex),
                                            class: "gt-xs"
                                          }, {
                                            default: _withCtx(() => [
                                              _createTextVNode(_toDisplayString(navMenuItem.title), 1)
                                            ]),
                                            _: 2
                                          }, 1032, ["href"]))
                                        : (navMenuItem.subscreens && navMenuItem.subscreens.length)
                                          ? (_openBlock(), _createElementBlock("div", {
                                              key: 1,
                                              class: "cursor-pointer gt-xs"
                                            }, [
                                              _createTextVNode(_toDisplayString(navMenuItem.title) + " ", 1),
                                              _createVNode(_component_q_menu, {
                                                anchor: "bottom left",
                                                self: "top left"
                                              }, {
                                                default: _withCtx(() => [
                                                  _createVNode(_component_q_list, {
                                                    dense: "",
                                                    style: {"min-width":"200px"}
                                                  }, {
                                                    default: _withCtx(() => [
                                                      (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(navMenuItem.subscreens, (subscreen) => {
                                                        return _withDirectives((_openBlock(), _createBlock(_component_q_item, {
                                                          key: subscreen.name,
                                                          class: _normalizeClass({'bg-primary':subscreen.active, 'text-white':subscreen.active}),
                                                          clickable: ""
                                                        }, {
                                                          default: _withCtx(() => [
                                                            _createVNode(_component_q_item_section, null, {
                                                              default: _withCtx(() => [
                                                                _createVNode(_component_m_link, {
                                                                  href: subscreen.pathWithParams
                                                                }, {
                                                                  default: _withCtx(() => [
                                                                    (subscreen.image)
                                                                      ? (_openBlock(), _createElementBlock(_Fragment, { key: 0 }, [
                                                                          (subscreen.imageType === 'icon')
                                                                            ? (_openBlock(), _createElementBlock("i", {
                                                                                key: 0,
                                                                                class: _normalizeClass(subscreen.image),
                                                                                style: {"padding-right":"4px"}
                                                                              }, null, 2))
                                                                            : (_openBlock(), _createElementBlock("img", {
                                                                                key: 1,
                                                                                src: subscreen.image,
                                                                                alt: subscreen.title,
                                                                                width: "18",
                                                                                class: "invertible",
                                                                                style: {"padding-right":"4px"}
                                                                              }, null, 8, ["src", "alt"]))
                                                                        ], 64))
                                                                      : (_openBlock(), _createElementBlock("i", {
                                                                          key: 1,
                                                                          class: "fa fa-link",
                                                                          style: {"padding-right":"8px"}
                                                                        })),
                                                                    _createTextVNode(" " + _toDisplayString(subscreen.title), 1)
                                                                  ]),
                                                                  _: 2
                                                                }, 1032, ["href"])
                                                              ]),
                                                              _: 2
                                                            }, 1024)
                                                          ]),
                                                          _: 2
                                                        }, 1032, ["class"])), [
                                                          [_directive_close_popup]
                                                        ])
                                                      }), 128))
                                                    ]),
                                                    _: 2
                                                  }, 1024)
                                                ]),
                                                _: 2
                                              }, 1024)
                                            ]))
                                          : (_openBlock(), _createBlock(_component_m_link, {
                                              key: 2,
                                              href: _ctx.getNavHref(menuIndex),
                                              class: "gt-xs"
                                            }, {
                                              default: _withCtx(() => [
                                                _createTextVNode(_toDisplayString(navMenuItem.title), 1)
                                              ]),
                                              _: 2
                                            }, 1032, ["href"])),
                                      _createVNode(_component_q_icon, {
                                        size: "1.5em",
                                        name: "chevron_right",
                                        color: "grey",
                                        class: "gt-xs"
                                      })
                                    ], 64))
                                  : _createCommentVNode("", true)
                              ], 64))
                            }), 256)),
                            (_ctx.navMenuList.length > 0)
                              ? (_openBlock(), _createBlock(_component_m_link, {
                                  key: 2,
                                  href: _ctx.getNavHref(_ctx.navMenuList.length - 1),
                                  class: "gt-xs"
                                }, {
                                  default: _withCtx(() => [
                                    _createTextVNode(_toDisplayString(_ctx.navMenuList[_ctx.navMenuList.length - 1].title), 1)
                                  ]),
                                  _: 1
                                }, 8, ["href"]))
                              : _createCommentVNode("", true),
                            _createVNode(_component_q_space),
                            _createVNode(_component_q_circular_progress, {
                              indeterminate: "",
                              size: "20px",
                              color: "light-blue",
                              class: _normalizeClass(["q-ma-xs", { hidden: _ctx.loading < 1 }])
                            }, null, 8, ["class"]),
                            (_openBlock(), _createBlock(_resolveDynamicComponent(_ctx.qzVue), { ref: "qzVue" }, null, 512)),
                            _createVNode(_component_q_btn, {
                              dense: "",
                              flat: "",
                              icon: "help_outline",
                              color: "info",
                              class: _normalizeClass({hidden:!_ctx.documentMenuList.length})
                            }, {
                              default: _withCtx(() => [
                                _createVNode(_component_q_tooltip, null, {
                                  default: _withCtx(() => [
                                    _createTextVNode(_toDisplayString(_ctx.labels.documentation), 1)
                                  ]),
                                  _: 1
                                }),
                                _createVNode(_component_q_menu, null, {
                                  default: _withCtx(() => [
                                    _createVNode(_component_q_list, {
                                      dense: "",
                                      class: "q-my-md"
                                    }, {
                                      default: _withCtx(() => [
                                        (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.documentMenuList, (screenDoc) => {
                                          return (_openBlock(), _createBlock(_component_q_item, {
                                            key: screenDoc.index
                                          }, {
                                            default: _withCtx(() => [
                                              _createVNode(_component_q_item_section, null, {
                                                default: _withCtx(() => [
                                                  _createVNode(_component_m_dynamic_dialog, {
                                                    url: _ctx.currentPath + '/screenDoc?docIndex=' + screenDoc.index,
                                                    "button-text": screenDoc.title,
                                                    title: screenDoc.title
                                                  }, null, 8, ["url", "button-text", "title"])
                                                ]),
                                                _: 2
                                              }, 1024)
                                            ]),
                                            _: 2
                                          }, 1024))
                                        }), 128))
                                      ]),
                                      _: 1
                                    })
                                  ]),
                                  _: 1
                                })
                              ]),
                              _: 1
                            }, 8, ["class"]),
                            (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.navPlugins, (navPlugin) => {
                              return (_openBlock(), _createBlock(_resolveDynamicComponent(navPlugin)))
                            }), 256)),
                            _createVNode(_component_q_btn, {
                              dense: "",
                              flat: "",
                              icon: "notifications"
                            }, {
                              default: _withCtx(() => [
                                _createVNode(_component_q_tooltip, null, {
                                  default: _withCtx(() => [
                                    _createTextVNode(_toDisplayString(_ctx.labels.notifyHistory), 1)
                                  ]),
                                  _: 1
                                }),
                                _createVNode(_component_q_menu, null, {
                                  default: _withCtx(() => [
                                    _createVNode(_component_q_list, {
                                      dense: "",
                                      style: {"min-width":"300px"}
                                    }, {
                                      default: _withCtx(() => [
                                        (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.notifyHistoryList, (histItem, histIndex) => {
                                          return (_openBlock(), _createBlock(_component_q_item, { key: histIndex }, {
                                            default: _withCtx(() => [
                                              _createVNode(_component_q_item_section, null, {
                                                default: _withCtx(() => [
                                                  _createVNode(_component_q_banner, {
                                                    dense: "",
                                                    rounded: "",
                                                    class: _normalizeClass(["text-white", 'bg-' + _ctx.getQuasarColor(histItem.type)])
                                                  }, _createSlots({
                                                    default: _withCtx(() => [
                                                      _createElementVNode("strong", null, _toDisplayString(histItem.time), 1),
                                                      _createTextVNode(),
                                                      _createElementVNode("span", null, _toDisplayString(histItem.message), 1)
                                                    ]),
                                                    _: 2
                                                  }, [
                                                    (histItem.link != null)
                                                      ? {
                                                          name: "action",
                                                          fn: _withCtx(() => [
                                                            _createVNode(_component_q_btn, {
                                                              to: histItem.link,
                                                              flat: "",
                                                              color: "white",
                                                              label: "View"
                                                            }, null, 8, ["to"])
                                                          ]),
                                                          key: "0"
                                                        }
                                                      : undefined
                                                  ]), 1032, ["class"])
                                                ]),
                                                _: 2
                                              }, 1024)
                                            ]),
                                            _: 2
                                          }, 1024))
                                        }), 128))
                                      ]),
                                      _: 1
                                    })
                                  ]),
                                  _: 1
                                })
                              ]),
                              _: 1
                            }),
                            _createVNode(_component_q_btn, {
                              dense: "",
                              flat: "",
                              icon: "history"
                            }, {
                              default: _withCtx(() => [
                                _createVNode(_component_q_tooltip, null, {
                                  default: _withCtx(() => [
                                    _createTextVNode(_toDisplayString(_ctx.labels.screenHistory), 1)
                                  ]),
                                  _: 1
                                }),
                                _createVNode(_component_q_menu, null, {
                                  default: _withCtx(() => [
                                    _createVNode(_component_q_list, {
                                      dense: "",
                                      style: {"min-width":"300px"}
                                    }, {
                                      default: _withCtx(() => [
                                        (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.navHistoryList, (histItem) => {
                                          return _withDirectives((_openBlock(), _createBlock(_component_q_item, {
                                            key: histItem.pathWithParams,
                                            clickable: ""
                                          }, {
                                            default: _withCtx(() => [
                                              _createVNode(_component_q_item_section, null, {
                                                default: _withCtx(() => [
                                                  _createVNode(_component_m_link, {
                                                    href: histItem.pathWithParams
                                                  }, {
                                                    default: _withCtx(() => [
                                                      (histItem.image)
                                                        ? (_openBlock(), _createElementBlock(_Fragment, { key: 0 }, [
                                                            (histItem.imageType === 'icon')
                                                              ? (_openBlock(), _createElementBlock("i", {
                                                                  key: 0,
                                                                  class: _normalizeClass(histItem.image),
                                                                  style: {"padding-right":"8px"}
                                                                }, null, 2))
                                                              : (_openBlock(), _createElementBlock("img", {
                                                                  key: 1,
                                                                  src: histItem.image,
                                                                  alt: histItem.title,
                                                                  width: "18",
                                                                  style: {"padding-right":"4px"}
                                                                }, null, 8, ["src", "alt"]))
                                                          ], 64))
                                                        : (_openBlock(), _createElementBlock("i", {
                                                            key: 1,
                                                            class: "fa fa-link",
                                                            style: {"padding-right":"8px"}
                                                          })),
                                                      _createTextVNode(" " + _toDisplayString(histItem.title), 1)
                                                    ]),
                                                    _: 2
                                                  }, 1032, ["href"])
                                                ]),
                                                _: 2
                                              }, 1024)
                                            ]),
                                            _: 2
                                          }, 1024)), [
                                            [_directive_close_popup]
                                          ])
                                        }), 128))
                                      ]),
                                      _: 1
                                    })
                                  ]),
                                  _: 1
                                })
                              ]),
                              _: 1
                            }),
                            _createVNode(_component_q_btn, {
                              dense: "",
                              flat: "",
                              icon: "account_circle"
                            }, {
                              default: _withCtx(() => [
                                _createVNode(_component_q_tooltip, null, {
                                  default: _withCtx(() => [
                                    _createTextVNode(_toDisplayString(_ctx.userFullName || _ctx.labels.account), 1)
                                  ]),
                                  _: 1
                                }),
                                _createVNode(_component_q_menu, null, {
                                  default: _withCtx(() => [
                                    _createVNode(_component_q_card, {
                                      flat: "",
                                      bordered: ""
                                    }, {
                                      default: _withCtx(() => [
                                        _createVNode(_component_q_card_section, {
                                          horizontal: "",
                                          class: "q-pa-md"
                                        }, {
                                          default: _withCtx(() => [
                                            _createVNode(_component_q_card_section, null, {
                                              default: _withCtx(() => [
                                                (_ctx.userFullName)
                                                  ? (_openBlock(), _createElementBlock("div", {
                                                      key: 0,
                                                      class: "q-mb-sm text-strong"
                                                    }, _toDisplayString(_ctx.labels.welcome) + " " + _toDisplayString(_ctx.userFullName), 1))
                                                  : _createCommentVNode("", true),
                                                (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.accountPlugins, (accountPlugin, accountIndex) => {
                                                  return (_openBlock(), _createBlock(_resolveDynamicComponent(accountPlugin), { key: accountIndex }))
                                                }), 128))
                                              ]),
                                              _: 1
                                            }),
                                            _createVNode(_component_q_separator, { vertical: "" }),
                                            _createVNode(_component_q_card_actions, {
                                              vertical: "",
                                              class: "justify-around q-px-md"
                                            }, {
                                              default: _withCtx(() => [
                                                _createVNode(_component_q_btn, {
                                                  flat: "",
                                                  dense: "",
                                                  icon: "settings_power",
                                                  color: "negative",
                                                  type: "a",
                                                  href: _ctx.shell.logoutUrl,
                                                  "data-confirm": _ctx.labels.logout + ' ' + _ctx.userFullName + '?'
                                                }, {
                                                  default: _withCtx(() => [
                                                    _createVNode(_component_q_tooltip, null, {
                                                      default: _withCtx(() => [
                                                        _createTextVNode(_toDisplayString(_ctx.labels.logout) + " " + _toDisplayString(_ctx.userFullName), 1)
                                                      ]),
                                                      _: 1
                                                    })
                                                  ]),
                                                  _: 1
                                                }, 8, ["href", "data-confirm"]),
                                                _createVNode(_component_q_btn, {
                                                  flat: "",
                                                  dense: "",
                                                  onClick: _withModifiers($event => (_ctx.switchDarkLight()), ["prevent"]),
                                                  icon: "invert_colors"
                                                }, {
                                                  default: _withCtx(() => [
                                                    _createVNode(_component_q_tooltip, null, {
                                                      default: _withCtx(() => [
                                                        _createTextVNode(_toDisplayString(_ctx.labels.switchDarkLight), 1)
                                                      ]),
                                                      _: 1
                                                    })
                                                  ]),
                                                  _: 1
                                                }, 8, ["onClick"]),
                                                _createVNode(_component_q_btn, {
                                                  flat: "",
                                                  dense: "",
                                                  icon: "autorenew",
                                                  color: "negative",
                                                  onClick: _ctx.reLoginShowDialog
                                                }, {
                                                  default: _withCtx(() => [
                                                    _createVNode(_component_q_tooltip, null, {
                                                      default: _withCtx(() => [
                                                        _createTextVNode("Re-Login")
                                                      ]),
                                                      _: 1
                                                    })
                                                  ]),
                                                  _: 1
                                                }, 8, ["onClick"])
                                              ]),
                                              _: 1
                                            })
                                          ]),
                                          _: 1
                                        })
                                      ]),
                                      _: 1
                                    })
                                  ]),
                                  _: 1
                                })
                              ]),
                              _: 1
                            })
                          ]),
                          _: 1
                        })
                      ]),
                      _: 1
                    }),
                    _createVNode(_component_q_page_container, null, {
                      default: _withCtx(() => [
                        _createElementVNode("div", { class: "shell-row" }, [
                          _withDirectives(_createElementVNode("div", {
                            class: _normalizeClass(["app-menu-col", { 'app-menu-overlay': _ctx.$q.screen.lt.md }])
                          }, [
                            _createVNode(_component_q_btn, {
                              dense: "",
                              flat: "",
                              icon: "menu",
                              onClick: $event => (_ctx.toggleLeftOpen()),
                              class: "lt-sm"
                            }, null, 8, ["onClick"]),
                            _createVNode(_component_q_list, {
                              dense: "",
                              padding: ""
                            }, {
                              default: _withCtx(() => [
                                _createVNode(_component_m_menu_nav_item, { "menu-index": 0 })
                              ]),
                              _: 1
                            })
                          ], 2), [
                            [_vShow, _ctx.leftOpen]
                          ]),
                          _createVNode(_component_q_page, { class: "shell-page-col" }, {
                            default: _withCtx(() => [
                              _createElementVNode("div", { class: "q-ma-sm" }, [
                                _createVNode(_component_m_subscreens_active)
                              ])
                            ]),
                            _: 1
                          })
                        ])
                      ]),
                      _: 1
                    }),
                    _createVNode(_component_q_footer, {
                      reveal: "",
                      bordered: "",
                      class: "bg-grey-9 text-white row q-pa-xs",
                      id: "footer"
                    }, {
                      default: _withCtx(() => [
                        _createElementVNode("div", {
                          innerHTML: _ctx.shell.footerHtml,
                          class: "row full-width"
                        }, null, 8, ["innerHTML"])
                      ]),
                      _: 1
                    })
                  ]),
                  _: 1
                }),
                _createVNode(_component_m_dialog, {
                  modelValue: _ctx.reLoginShow,
                  "onUpdate:modelValue": $event => ((_ctx.reLoginShow) = $event),
                  width: "400",
                  title: _ctx.labels.reLogin
                }, {
                  default: _withCtx(() => [
                    (_ctx.reLoginMfaData)
                      ? (_openBlock(), _createElementBlock("div", { key: 0 }, [
                          _createElementVNode("div", { style: {"text-align":"center","padding-bottom":"10px"} }, [
                            _createTextVNode("User "),
                            _createElementVNode("strong", null, _toDisplayString(_ctx.username), 1),
                            _createTextVNode(" requires an authentication code, you have these options:")
                          ]),
                          _createElementVNode("div", { style: {"text-align":"center","padding-bottom":"10px"} }, _toDisplayString(_ctx.reLoginMfaData.factorTypeDescriptions.join(", ")), 1),
                          _createVNode(_component_q_form, {
                            onSubmit: _withModifiers(_ctx.reLoginVerifyOtp, ["prevent"]),
                            autocapitalize: "off",
                            autocomplete: "off"
                          }, {
                            default: _withCtx(() => [
                              _createVNode(_component_m_text_line, {
                                modelValue: _ctx.reLoginOtp,
                                "onUpdate:modelValue": $event => ((_ctx.reLoginOtp) = $event),
                                name: "code",
                                type: "password",
                                label: _ctx.labels.authenticationCode
                              }, null, 8, ["modelValue", "onUpdate:modelValue", "label"]),
                              _createVNode(_component_q_btn, {
                                outline: "",
                                "no-caps": "",
                                color: "primary",
                                type: "submit",
                                label: _ctx.labels.signIn
                              }, null, 8, ["label"])
                            ]),
                            _: 1
                          }, 8, ["onSubmit"]),
                          (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.reLoginMfaData.sendableFactors, (sendableFactor) => {
                            return (_openBlock(), _createElementBlock("div", {
                              key: sendableFactor.factorId,
                              style: {"padding":"8px"}
                            }, [
                              _createVNode(_component_q_btn, {
                                outline: "",
                                "no-caps": "",
                                dense: "",
                                label: _ctx.labels.sendCodeTo + ' ' + sendableFactor.factorOption,
                                onClick: _withModifiers($event => (_ctx.reLoginSendOtp(sendableFactor.factorId)), ["prevent"])
                              }, null, 8, ["label", "onClick"])
                            ]))
                          }), 128))
                        ]))
                      : (_openBlock(), _createElementBlock("div", { key: 1 }, [
                          _createElementVNode("div", { style: {"text-align":"center","padding-bottom":"10px"} }, [
                            _createTextVNode("Please sign in to continue as user "),
                            _createElementVNode("strong", null, _toDisplayString(_ctx.username), 1)
                          ]),
                          _createVNode(_component_q_form, {
                            onSubmit: _withModifiers(_ctx.reLoginSubmit, ["prevent"]),
                            autocapitalize: "off",
                            autocomplete: "off"
                          }, {
                            default: _withCtx(() => [
                              _createVNode(_component_m_text_line, {
                                modelValue: _ctx.reLoginPassword,
                                "onUpdate:modelValue": $event => ((_ctx.reLoginPassword) = $event),
                                name: "password",
                                type: "password",
                                label: _ctx.labels.password
                              }, null, 8, ["modelValue", "onUpdate:modelValue", "label"]),
                              _createVNode(_component_q_btn, {
                                outline: "",
                                "no-caps": "",
                                color: "primary",
                                type: "submit",
                                label: _ctx.labels.signIn
                              }, null, 8, ["label"]),
                              _createVNode(_component_q_btn, {
                                outline: "",
                                "no-caps": "",
                                color: "negative",
                                onClick: _withModifiers(_ctx.reLoginReload, ["prevent"]),
                                label: _ctx.labels.reloadPage
                              }, null, 8, ["onClick", "label"])
                            ]),
                            _: 1
                          }, 8, ["onSubmit"])
                        ]))
                  ]),
                  _: 1
                }, 8, ["modelValue", "onUpdate:modelValue", "title"])
              ], 64))
            }
        } },
        "m-radio": { hash: "e993f4fe", make: function (Vue) {
            const { toDisplayString: _toDisplayString, createTextVNode: _createTextVNode, resolveComponent: _resolveComponent, withCtx: _withCtx, openBlock: _openBlock, createBlock: _createBlock, createCommentVNode: _createCommentVNode, renderList: _renderList, Fragment: _Fragment, createElementBlock: _createElementBlock, mergeProps: _mergeProps } = Vue

            return function render(_ctx, _cache) {
              const _component_q_tooltip = _resolveComponent("q-tooltip")
              const _component_q_radio = _resolveComponent("q-radio")
              const _component_m_field = _resolveComponent("m-field")

              return (_openBlock(), _createBlock(_component_m_field, _mergeProps({
                label: _ctx.label,
                "bg-color": _ctx.bg,
                disable: _ctx.disable
              }, _ctx.errorAttrs), {
                default: _withCtx(() => [
                  (_ctx.tooltip)
                    ? (_openBlock(), _createBlock(_component_q_tooltip, { key: 0 }, {
                        default: _withCtx(() => [
                          _createTextVNode(_toDisplayString(_ctx.tooltip), 1)
                        ]),
                        _: 1
                      }))
                    : _createCommentVNode("", true),
                  (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.options, (option, index) => {
                    return (_openBlock(), _createBlock(_component_q_radio, {
                      key: option.value,
                      size: "xs",
                      val: option.value,
                      label: option.label,
                      "model-value": _ctx.local,
                      "onUpdate:modelValue": _ctx.onInput,
                      name: _ctx.name,
                      id: index > 0 ? _ctx.id + '_' + index : _ctx.id,
                      form: _ctx.form,
                      disable: _ctx.disable
                    }, null, 8, ["val", "label", "model-value", "onUpdate:modelValue", "name", "id", "form", "disable"]))
                  }), 128))
                ]),
                _: 1
              }, 16, ["label", "bg-color", "disable"]))
            }
        } },
        "m-range-find": { hash: "c79ddf98", make: function (Vue) {
            const { resolveComponent: _resolveComponent, createVNode: _createVNode, openBlock: _openBlock, createElementBlock: _createElementBlock } = Vue

            return function render(_ctx, _cache) {
              const _component_m_text_line = _resolveComponent("m-text-line")

              return (_openBlock(), _createElementBlock("div", { class: "row" }, [
                _createVNode(_component_m_text_line, {
                  label: _ctx.label + ' From',
                  name: _ctx.name + '_from',
                  id: _ctx.id + '_from',
                  size: _ctx.size,
                  maxlength: _ctx.maxlength,
                  form: _ctx.form,
                  tooltip: _ctx.tooltip,
                  modelValue: _ctx.fields[_ctx.name + '_from'],
                  "onUpdate:modelValue": $event => ((_ctx.fields[_ctx.name + '_from']) = $event)
                }, null, 8, ["label", "name", "id", "size", "maxlength", "form", "tooltip", "modelValue", "onUpdate:modelValue"]),
                _createVNode(_component_m_text_line, {
                  class: "q-pl-xs",
                  label: _ctx.label + ' Thru',
                  name: _ctx.name + '_thru',
                  id: _ctx.id + '_thru',
                  size: _ctx.size,
                  maxlength: _ctx.maxlength,
                  form: _ctx.form,
                  tooltip: _ctx.tooltip,
                  modelValue: _ctx.fields[_ctx.name + '_thru'],
                  "onUpdate:modelValue": $event => ((_ctx.fields[_ctx.name + '_thru']) = $event)
                }, null, 8, ["label", "name", "id", "size", "maxlength", "form", "tooltip", "modelValue", "onUpdate:modelValue"])
              ]))
            }
        } },
        "m-script": { hash: "ddf09c45", make: function (Vue) {
            const { renderSlot: _renderSlot, openBlock: _openBlock, createElementBlock: _createElementBlock } = Vue

            return function render(_ctx, _cache) {
              return (_openBlock(), _createElementBlock("div", {
                type: _ctx.type,
                style: {"display":"none"}
              }, [
                _renderSlot(_ctx.$slots, "default")
              ], 8, ["type"]))
            }
        } },
        "m-stylesheet": { hash: "e8119ca2", make: function (Vue) {
            const { openBlock: _openBlock, createElementBlock: _createElementBlock } = Vue

            return function render(_ctx, _cache) {
              return (_openBlock(), _createElementBlock("div", {
                type: _ctx.type,
                style: {"display":"none"}
              }, null, 8, ["type"]))
            }
        } },
        "m-subscreens-active": { hash: "49acb545", make: function (Vue) {
            const { resolveDynamicComponent: _resolveDynamicComponent, openBlock: _openBlock, createBlock: _createBlock } = Vue

            return function render(_ctx, _cache) {
              return (_openBlock(), _createBlock(_resolveDynamicComponent(_ctx.activeComponent), { style: {"height":"100%","width":"100%"} }))
            }
        } },
        "m-subscreens-menu": { hash: "d6eb2f82", make: function (Vue) {
            const { renderList: _renderList, Fragment: _Fragment, openBlock: _openBlock, createElementBlock: _createElementBlock, resolveComponent: _resolveComponent, withModifiers: _withModifiers, createBlock: _createBlock, withCtx: _withCtx, createVNode: _createVNode, createCommentVNode: _createCommentVNode } = Vue

            return function render(_ctx, _cache) {
              const _component_q_tab = _resolveComponent("q-tab")
              const _component_q_tabs = _resolveComponent("q-tabs")
              const _component_q_separator = _resolveComponent("q-separator")

              return (_ctx.subscreens.length > 1)
                ? (_openBlock(), _createElementBlock("div", { key: 0 }, [
                    _createVNode(_component_q_tabs, {
                      dense: "",
                      "no-caps": "",
                      align: "left",
                      "active-color": "primary",
                      "indicator-color": "primary",
                      "model-value": _ctx.activeTab
                    }, {
                      default: _withCtx(() => [
                        (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.subscreens, (tab) => {
                          return (_openBlock(), _createBlock(_component_q_tab, {
                            key: tab.name,
                            name: tab.name,
                            label: tab.title,
                            disable: tab.disableLink,
                            onClick: _withModifiers($event => (_ctx.goTo(tab.pathWithParams)), ["prevent"])
                          }, null, 8, ["name", "label", "disable", "onClick"]))
                        }), 128))
                      ]),
                      _: 1
                    }, 8, ["model-value"]),
                    _createVNode(_component_q_separator, { class: "q-mb-md" })
                  ]))
                : _createCommentVNode("", true)
            }
        } },
        "m-subscreens-panel": { hash: "3c7bfb94", make: function (Vue) {
            const { resolveComponent: _resolveComponent, createVNode: _createVNode, openBlock: _openBlock, createElementBlock: _createElementBlock } = Vue

            return function render(_ctx, _cache) {
              const _component_m_subscreens_tabs = _resolveComponent("m-subscreens-tabs")
              const _component_m_subscreens_active = _resolveComponent("m-subscreens-active")

              return (_openBlock(), _createElementBlock("div", null, [
                _createVNode(_component_m_subscreens_tabs),
                _createVNode(_component_m_subscreens_active)
              ]))
            }
        } },
        "m-text-area": { hash: "afc3b19d", make: function (Vue) {
            const { toDisplayString: _toDisplayString, openBlock: _openBlock, createElementBlock: _createElementBlock, createCommentVNode: _createCommentVNode, resolveComponent: _resolveComponent, createVNode: _createVNode, createElementVNode: _createElementVNode, createTextVNode: _createTextVNode, withCtx: _withCtx, mergeProps: _mergeProps, createBlock: _createBlock } = Vue

            return function render(_ctx, _cache) {
              const _component_q_editor = _resolveComponent("q-editor")
              const _component_q_tooltip = _resolveComponent("q-tooltip")
              const _component_q_btn = _resolveComponent("q-btn")
              const _component_q_input = _resolveComponent("q-input")

              return (_ctx.editorType === 'html')
                ? (_openBlock(), _createElementBlock("div", {
                    key: 0,
                    class: "m-text-area-editor"
                  }, [
                    (_ctx.label)
                      ? (_openBlock(), _createElementBlock("div", {
                          key: 0,
                          class: "text-caption"
                        }, _toDisplayString(_ctx.label), 1))
                      : _createCommentVNode("", true),
                    _createVNode(_component_q_editor, {
                      "model-value": _ctx.local,
                      "onUpdate:modelValue": _ctx.onInput,
                      toolbar: _ctx.toolbar,
                      "min-height": "12rem",
                      readonly: _ctx.readonly,
                      disable: _ctx.disable
                    }, null, 8, ["model-value", "onUpdate:modelValue", "toolbar", "readonly", "disable"]),
                    _createElementVNode("input", {
                      type: "hidden",
                      name: _ctx.name,
                      value: _ctx.local,
                      form: _ctx.form
                    }, null, 8, ["name", "value", "form"])
                  ]))
                : (_ctx.editorType === 'md')
                  ? (_openBlock(), _createElementBlock("div", {
                      key: 1,
                      class: "m-text-area-editor"
                    }, [
                      _createVNode(_component_q_input, _mergeProps({
                        type: "textarea",
                        dense: "",
                        outlined: "",
                        "stack-label": "",
                        label: _ctx.label,
                        "model-value": _ctx.local,
                        "onUpdate:modelValue": _ctx.onInput,
                        name: _ctx.name,
                        for: _ctx.id,
                        rows: _ctx.rows,
                        readonly: _ctx.readonly,
                        disable: _ctx.disable,
                        form: _ctx.form,
                        "bg-color": _ctx.bg,
                        autogrow: ""
                      }, _ctx.errorAttrs), {
                        after: _withCtx(() => [
                          _createVNode(_component_q_btn, {
                            dense: "",
                            flat: "",
                            icon: _ctx.preview ? 'edit' : 'visibility',
                            onClick: _ctx.togglePreview
                          }, {
                            default: _withCtx(() => [
                              _createVNode(_component_q_tooltip, null, {
                                default: _withCtx(() => [
                                  _createTextVNode("Preview")
                                ]),
                                _: 1
                              })
                            ]),
                            _: 1
                          }, 8, ["icon", "onClick"])
                        ]),
                        _: 1
                      }, 16, ["label", "model-value", "onUpdate:modelValue", "name", "for", "rows", "readonly", "disable", "form", "bg-color"]),
                      (_ctx.preview)
                        ? (_openBlock(), _createElementBlock("div", {
                            key: 0,
                            class: "q-pa-sm markdown-preview",
                            innerHTML: _ctx.previewHtml
                          }, null, 8, ["innerHTML"]))
                        : _createCommentVNode("", true)
                    ]))
                  : (_openBlock(), _createBlock(_component_q_input, _mergeProps({
                      key: 2,
                      type: "textarea",
                      dense: "",
                      outlined: "",
                      "stack-label": "",
                      label: _ctx.label,
                      "model-value": _ctx.local,
                      "onUpdate:modelValue": _ctx.onInput,
                      name: _ctx.name,
                      for: _ctx.id,
                      rows: _ctx.rows,
                      cols: _ctx.cols,
                      readonly: _ctx.readonly,
                      autogrow: _ctx.autogrow,
                      maxlength: _ctx.maxlength,
                      disable: _ctx.disable,
                      form: _ctx.form,
                      "bg-color": _ctx.bg
                    }, _ctx.errorAttrs), {
                      default: _withCtx(() => [
                        (_ctx.tooltip)
                          ? (_openBlock(), _createBlock(_component_q_tooltip, { key: 0 }, {
                              default: _withCtx(() => [
                                _createTextVNode(_toDisplayString(_ctx.tooltip), 1)
                              ]),
                              _: 1
                            }))
                          : _createCommentVNode("", true)
                      ]),
                      _: 1
                    }, 16, ["label", "model-value", "onUpdate:modelValue", "name", "for", "rows", "cols", "readonly", "autogrow", "maxlength", "disable", "form", "bg-color"]))
            }
        } },
        "m-text-find": { hash: "f6d79dd1", make: function (Vue) {
            const { resolveComponent: _resolveComponent, createVNode: _createVNode, openBlock: _openBlock, createBlock: _createBlock, createCommentVNode: _createCommentVNode, createElementBlock: _createElementBlock, Fragment: _Fragment } = Vue

            return function render(_ctx, _cache) {
              const _component_m_text_line = _resolveComponent("m-text-line")
              const _component_q_checkbox = _resolveComponent("q-checkbox")
              const _component_q_select = _resolveComponent("q-select")

              return (_openBlock(), _createElementBlock(_Fragment, null, [
                _createVNode(_component_m_text_line, {
                  label: _ctx.label,
                  name: _ctx.name,
                  id: _ctx.id,
                  size: _ctx.size,
                  maxlength: _ctx.maxlength,
                  form: _ctx.form,
                  tooltip: _ctx.tooltip,
                  modelValue: _ctx.fields[_ctx.name],
                  "onUpdate:modelValue": $event => ((_ctx.fields[_ctx.name]) = $event)
                }, null, 8, ["label", "name", "id", "size", "maxlength", "form", "tooltip", "modelValue", "onUpdate:modelValue"]),
                (_ctx.hideOptions !== 'true')
                  ? (_openBlock(), _createElementBlock("div", {
                      key: 0,
                      class: "row items-center text-find-options"
                    }, [
                      (_ctx.hideOptions !== 'operator')
                        ? (_openBlock(), _createBlock(_component_q_checkbox, {
                            key: 0,
                            size: "xs",
                            name: _ctx.name + '_not',
                            label: "Not",
                            "true-value": "Y",
                            "false-value": "N",
                            modelValue: _ctx.fields[_ctx.name + '_not'],
                            "onUpdate:modelValue": $event => ((_ctx.fields[_ctx.name + '_not']) = $event),
                            form: _ctx.form
                          }, null, 8, ["name", "modelValue", "onUpdate:modelValue", "form"]))
                        : _createCommentVNode("", true),
                      (_ctx.hideOptions !== 'operator')
                        ? (_openBlock(), _createBlock(_component_q_select, {
                            key: 1,
                            dense: "",
                            outlined: "",
                            "options-dense": "",
                            "emit-value": "",
                            "map-options": "",
                            name: _ctx.name + '_op',
                            modelValue: _ctx.fields[_ctx.name + '_op'],
                            "onUpdate:modelValue": $event => ((_ctx.fields[_ctx.name + '_op']) = $event),
                            options: _ctx.operators,
                            form: _ctx.form,
                            behavior: "menu"
                          }, null, 8, ["name", "modelValue", "onUpdate:modelValue", "options", "form"]))
                        : _createCommentVNode("", true),
                      (_ctx.hideOptions !== 'ignore-case')
                        ? (_openBlock(), _createBlock(_component_q_checkbox, {
                            key: 2,
                            size: "xs",
                            name: _ctx.name + '_ic',
                            label: "Ignore Case",
                            "true-value": "Y",
                            "false-value": "N",
                            modelValue: _ctx.fields[_ctx.name + '_ic'],
                            "onUpdate:modelValue": $event => ((_ctx.fields[_ctx.name + '_ic']) = $event),
                            form: _ctx.form
                          }, null, 8, ["name", "modelValue", "onUpdate:modelValue", "form"]))
                        : _createCommentVNode("", true)
                    ]))
                  : _createCommentVNode("", true)
              ], 64))
            }
        } },
        "m-text-line": { hash: "8e3a5bca", make: function (Vue) {
            const { toDisplayString: _toDisplayString, createTextVNode: _createTextVNode, resolveComponent: _resolveComponent, withCtx: _withCtx, openBlock: _openBlock, createBlock: _createBlock, createCommentVNode: _createCommentVNode, mergeProps: _mergeProps } = Vue

            return function render(_ctx, _cache) {
              const _component_q_tooltip = _resolveComponent("q-tooltip")
              const _component_q_input = _resolveComponent("q-input")

              return (_openBlock(), _createBlock(_component_q_input, _mergeProps({
                dense: "",
                outlined: "",
                "stack-label": "",
                label: _ctx.label,
                prefix: _ctx.prefix,
                "model-value": _ctx.local,
                "onUpdate:modelValue": _ctx.onInput,
                type: _ctx.type,
                for: _ctx.id,
                name: _ctx.name,
                size: _ctx.size,
                maxlength: _ctx.maxlength,
                form: _ctx.form,
                loading: _ctx.loading,
                rules: _ctx.allRules,
                disable: _ctx.disable,
                "bg-color": _ctx.bg,
                mask: _ctx.mask,
                "fill-mask": _ctx.fillMask,
                "reverse-fill-mask": _ctx.reverseFillMask,
                autocapitalize: "off",
                autocomplete: "off"
              }, _ctx.errorAttrs), {
                default: _withCtx(() => [
                  (_ctx.tooltip)
                    ? (_openBlock(), _createBlock(_component_q_tooltip, { key: 0 }, {
                        default: _withCtx(() => [
                          _createTextVNode(_toDisplayString(_ctx.tooltip), 1)
                        ]),
                        _: 1
                      }))
                    : _createCommentVNode("", true)
                ]),
                _: 1
              }, 16, ["label", "prefix", "model-value", "onUpdate:modelValue", "type", "for", "name", "size", "maxlength", "form", "loading", "rules", "disable", "bg-color", "mask", "fill-mask", "reverse-fill-mask"]))
            }
        } },
        "m-tree-item": { hash: "b701718a", make: function (Vue) {
            const { normalizeClass: _normalizeClass, openBlock: _openBlock, createElementBlock: _createElementBlock, createCommentVNode: _createCommentVNode, toDisplayString: _toDisplayString, createTextVNode: _createTextVNode, resolveComponent: _resolveComponent, withCtx: _withCtx, createBlock: _createBlock, createElementVNode: _createElementVNode, renderList: _renderList, Fragment: _Fragment, vShow: _vShow, withDirectives: _withDirectives } = Vue

            return function render(_ctx, _cache) {
              const _component_m_link = _resolveComponent("m-link")
              const _component_m_tree_item = _resolveComponent("m-tree-item")

              return (_openBlock(), _createElementBlock("li", {
                id: _ctx.model.id
              }, [
                (_ctx.isFolder)
                  ? (_openBlock(), _createElementBlock("i", {
                      key: 0,
                      onClick: _ctx.toggle,
                      class: _normalizeClass(["fa", {'fa-chevron-right':!_ctx.open, 'fa-chevron-down':_ctx.open}])
                    }, null, 10, ["onClick"]))
                  : (_openBlock(), _createElementBlock("i", {
                      key: 1,
                      class: "fa fa-square-o"
                    })),
                _createTextVNode(),
                _createElementVNode("span", { onClick: _ctx.setSelected }, [
                  (_ctx.linkHref)
                    ? (_openBlock(), _createBlock(_component_m_link, {
                        key: 0,
                        href: _ctx.linkHref,
                        "load-id": _ctx.model.a_attr.loadId,
                        class: _normalizeClass({'text-success':_ctx.selected})
                      }, {
                        default: _withCtx(() => [
                          _createTextVNode(_toDisplayString(_ctx.model.text), 1)
                        ]),
                        _: 1
                      }, 8, ["href", "load-id", "class"]))
                    : (_openBlock(), _createElementBlock("span", {
                        key: 1,
                        class: _normalizeClass({'text-success':_ctx.selected})
                      }, _toDisplayString(_ctx.model.text), 3))
                ], 8, ["onClick"]),
                (_ctx.hasChildren)
                  ? _withDirectives((_openBlock(), _createElementBlock("ul", { key: 2 }, [
                      (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.model.children, (child) => {
                        return (_openBlock(), _createBlock(_component_m_tree_item, {
                          key: child.id,
                          model: child,
                          top: _ctx.top
                        }, null, 8, ["model", "top"]))
                      }), 128))
                    ], 512)), [
                      [_vShow, _ctx.open]
                    ])
                  : _createCommentVNode("", true)
              ], 8, ["id"]))
            }
        } },
        "m-tree-top": { hash: "14ff4400", make: function (Vue) {
            const { renderList: _renderList, Fragment: _Fragment, openBlock: _openBlock, createElementBlock: _createElementBlock, resolveComponent: _resolveComponent, createBlock: _createBlock } = Vue

            return function render(_ctx, _cache) {
              const _component_m_tree_item = _resolveComponent("m-tree-item")

              return (_openBlock(), _createElementBlock("ul", {
                id: _ctx.id,
                class: "tree-list"
              }, [
                (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.itemList, (model) => {
                  return (_openBlock(), _createBlock(_component_m_tree_item, {
                    key: model.id,
                    model: model,
                    top: _ctx.top
                  }, null, 8, ["model", "top"]))
                }), 128))
              ], 8, ["id"]))
            }
        } }
    };
})(globalThis.moqui);

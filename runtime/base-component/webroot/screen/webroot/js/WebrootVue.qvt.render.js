/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

// Generated from the component templates of WebrootVue.qvt.js; do not edit. Make it again with
//   UPDATE_GOLDEN=1 cargo test -p moqui-core --lib client_components_are_precompiled
// qvt.applyRenders uses an entry only while its hash matches the template (plans/archive/QvtServerStatic.md, Page build).
(function (moqui) {
    "use strict";
    moqui.qvtRenders = {
        "m-assist-button": { hash: "aab4ba2b", make: function (Vue) {
            const { toDisplayString: _toDisplayString, createTextVNode: _createTextVNode, resolveComponent: _resolveComponent, withCtx: _withCtx, openBlock: _openBlock, createBlock: _createBlock, createCommentVNode: _createCommentVNode, createVNode: _createVNode, normalizeClass: _normalizeClass } = Vue

            return function render(_ctx, _cache) {
              const _component_q_badge = _resolveComponent("q-badge")
              const _component_q_tooltip = _resolveComponent("q-tooltip")
              const _component_q_btn = _resolveComponent("q-btn")

              return (_openBlock(), _createBlock(_component_q_btn, {
                flat: "",
                round: "",
                dense: "",
                id: "m-assist-button",
                icon: "sym_o_auto_awesome",
                class: _normalizeClass({'text-primary': _ctx.open}),
                "aria-label": _ctx.labels.assist,
                "aria-expanded": _ctx.open ? 'true' : 'false',
                onClick: _ctx.toggle
              }, {
                default: _withCtx(() => [
                  (_ctx.count)
                    ? (_openBlock(), _createBlock(_component_q_badge, {
                        key: 0,
                        floating: "",
                        rounded: "",
                        color: "negative",
                        "data-assist-badge": ""
                      }, {
                        default: _withCtx(() => [
                          _createTextVNode(_toDisplayString(_ctx.count > 99 ? '99+' : _ctx.count), 1)
                        ]),
                        _: 1
                      }))
                    : _createCommentVNode("", true),
                  _createVNode(_component_q_tooltip, null, {
                    default: _withCtx(() => [
                      _createTextVNode(_toDisplayString(_ctx.labels.assist) + " (" + _toDisplayString(_ctx.$root.paletteKeyHint ? _ctx.$root.paletteKeyHint.replace('K', 'J') : 'Ctrl J') + ")", 1)
                    ]),
                    _: 1
                  })
                ]),
                _: 1
              }, 8, ["class", "aria-label", "aria-expanded", "onClick"]))
            }
        } },
        "m-assist-canvas": { hash: "4b56984", make: function (Vue) {
            const { resolveComponent: _resolveComponent, createVNode: _createVNode, toDisplayString: _toDisplayString, createElementVNode: _createElementVNode, createTextVNode: _createTextVNode, withCtx: _withCtx, openBlock: _openBlock, createBlock: _createBlock, createCommentVNode: _createCommentVNode, createElementBlock: _createElementBlock, resolveDynamicComponent: _resolveDynamicComponent, renderList: _renderList, Fragment: _Fragment, withModifiers: _withModifiers, withKeys: _withKeys } = Vue

            return function render(_ctx, _cache) {
              const _component_q_icon = _resolveComponent("q-icon")
              const _component_q_badge = _resolveComponent("q-badge")
              const _component_q_spinner_dots = _resolveComponent("q-spinner-dots")
              const _component_q_space = _resolveComponent("q-space")
              const _component_q_btn = _resolveComponent("q-btn")
              const _component_q_input = _resolveComponent("q-input")

              return (_ctx.shown)
                ? (_openBlock(), _createElementBlock("div", {
                    key: 0,
                    class: "m-assist-canvas q-pa-md",
                    id: "m-assist-canvas",
                    "data-assist-canvas": "",
                    "data-version": _ctx.view.version,
                    "data-streaming": _ctx.view.streaming ? 'true' : undefined
                  }, [
                    _createElementVNode("div", { class: "row items-center no-wrap q-mb-sm m-assist-canvas-bar" }, [
                      _createVNode(_component_q_icon, {
                        name: "sym_o_dashboard",
                        size: "sm",
                        class: "q-mr-sm text-primary"
                      }),
                      _createElementVNode("div", {
                        class: "text-h6 ellipsis",
                        id: "m-assist-canvas-title"
                      }, _toDisplayString(_ctx.view.title || _ctx.labels.canvas), 1),
                      _createVNode(_component_q_badge, {
                        outline: "",
                        color: "primary",
                        class: "q-ml-sm"
                      }, {
                        default: _withCtx(() => [
                          _createTextVNode("v" + _toDisplayString(_ctx.view.version), 1)
                        ]),
                        _: 1
                      }),
                      (_ctx.view.streaming || _ctx.view.updating)
                        ? (_openBlock(), _createBlock(_component_q_spinner_dots, {
                            key: 0,
                            size: "18px",
                            class: "q-ml-sm",
                            "aria-label": _ctx.labels.canvasUpdating
                          }, null, 8, ["aria-label"]))
                        : _createCommentVNode("", true),
                      _createVNode(_component_q_space),
                      _createVNode(_component_q_btn, {
                        flat: "",
                        dense: "",
                        "no-caps": "",
                        icon: "sym_o_arrow_back",
                        label: _ctx.labels.backToPage,
                        id: "m-assist-canvas-back",
                        onClick: _ctx.back
                      }, null, 8, ["label", "onClick"]),
                      _createVNode(_component_q_btn, {
                        flat: "",
                        round: "",
                        dense: "",
                        icon: "sym_o_close",
                        id: "m-assist-canvas-close",
                        "aria-label": _ctx.labels.closeCanvas,
                        onClick: _ctx.close
                      }, null, 8, ["aria-label", "onClick"])
                    ]),
                    (!_ctx.s.canvasLoaded || !_ctx.dialect)
                      ? (_openBlock(), _createElementBlock("div", {
                          key: 0,
                          class: "q-pa-md m-muted"
                        }, _toDisplayString(_ctx.s.canvasError || _ctx.labels.loading), 1))
                      : (_openBlock(), _createBlock(_resolveDynamicComponent(_ctx.dialect), {
                          key: 1,
                          program: _ctx.view.program,
                          streaming: _ctx.view.streaming,
                          "artifact-id": _ctx.view.artifactId,
                          version: _ctx.view.version,
                          "initial-state": _ctx.view.state,
                          onProblems: _ctx.onProblems,
                          onState: _ctx.onState,
                          onAction: _ctx.onAction,
                          onNavigate: _ctx.onNavigate
                        }, null, 40, ["program", "streaming", "artifact-id", "version", "initial-state", "onProblems", "onState", "onAction", "onNavigate"])),
                    (_ctx.problems.length)
                      ? (_openBlock(), _createElementBlock("div", {
                          key: 2,
                          class: "q-mt-md m-assist-canvas-problems",
                          id: "m-assist-canvas-problems"
                        }, [
                          _createElementVNode("div", { class: "text-caption text-negative" }, _toDisplayString(_ctx.labels.canvasProblems), 1),
                          (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.problems, (problem, index) => {
                            return (_openBlock(), _createElementBlock("div", {
                              key: index,
                              class: "text-caption m-muted",
                              "data-assist-canvas-problem": ""
                            }, _toDisplayString(problem.problem) + " (" + _toDisplayString(problem.skill) + ")", 1))
                          }), 128))
                        ]))
                      : _createCommentVNode("", true),
                    _createElementVNode("div", { class: "row items-center no-wrap q-mt-md q-gutter-sm m-assist-canvas-adjust" }, [
                      _createVNode(_component_q_input, {
                        dense: "",
                        outlined: "",
                        modelValue: _ctx.adjust,
                        "onUpdate:modelValue": $event => ((_ctx.adjust) = $event),
                        placeholder: _ctx.labels.adjust,
                        class: "col",
                        for: "m-assist-canvas-adjust",
                        onKeydown: _withKeys(_withModifiers(_ctx.sendAdjust, ["prevent"]), ["enter"])
                      }, null, 8, ["modelValue", "onUpdate:modelValue", "placeholder", "onKeydown"]),
                      _createVNode(_component_q_btn, {
                        flat: "",
                        dense: "",
                        "no-caps": "",
                        color: "primary",
                        label: _ctx.labels.adjustSend,
                        id: "m-assist-canvas-adjust-send",
                        disable: !_ctx.adjust.trim(),
                        onClick: _ctx.sendAdjust
                      }, null, 8, ["label", "disable", "onClick"])
                    ])
                  ], 8, ["data-version", "data-streaming"]))
                : _createCommentVNode("", true)
            }
        } },
        "m-assist-card": { hash: "dfb695cf", make: function (Vue) {
            const { resolveComponent: _resolveComponent, createVNode: _createVNode, toDisplayString: _toDisplayString, createElementVNode: _createElementVNode, createTextVNode: _createTextVNode, withCtx: _withCtx, openBlock: _openBlock, createBlock: _createBlock, createCommentVNode: _createCommentVNode, createElementBlock: _createElementBlock, normalizeClass: _normalizeClass, renderList: _renderList, Fragment: _Fragment, withKeys: _withKeys } = Vue

            return function render(_ctx, _cache) {
              const _component_q_icon = _resolveComponent("q-icon")
              const _component_q_space = _resolveComponent("q-space")
              const _component_q_badge = _resolveComponent("q-badge")
              const _component_q_chip = _resolveComponent("q-chip")
              const _component_q_input = _resolveComponent("q-input")
              const _component_q_card_section = _resolveComponent("q-card-section")
              const _component_q_btn = _resolveComponent("q-btn")
              const _component_q_tooltip = _resolveComponent("q-tooltip")
              const _component_q_card_actions = _resolveComponent("q-card-actions")
              const _component_q_select = _resolveComponent("q-select")
              const _component_q_checkbox = _resolveComponent("q-checkbox")
              const _component_q_card = _resolveComponent("q-card")

              return (_openBlock(), _createBlock(_component_q_card, {
                flat: "",
                bordered: "",
                class: "m-assist-card q-mb-sm",
                "data-assist-card": _ctx.kind,
                "data-status": _ctx.status,
                "data-approval-id": _ctx.card.approvalId,
                "data-question-id": _ctx.card.questionId,
                "data-handoff-id": _ctx.card.handoffId
              }, {
                default: _withCtx(() => [
                  (_ctx.kind === 'approval')
                    ? (_openBlock(), _createElementBlock(_Fragment, { key: 0 }, [
                        _createVNode(_component_q_card_section, { class: "q-pa-sm" }, {
                          default: _withCtx(() => [
                            _createElementVNode("div", { class: "row items-center no-wrap" }, [
                              _createVNode(_component_q_icon, {
                                name: _ctx.card.classification === 'destructive' ? 'sym_o_warning' : 'sym_o_approval',
                                color: _ctx.card.classification === 'destructive' ? 'negative' : 'primary',
                                size: "xs",
                                class: "q-mr-xs"
                              }, null, 8, ["name", "color"]),
                              _createElementVNode("div", { class: "text-weight-medium ellipsis" }, _toDisplayString(_ctx.card.summary || _ctx.card.targetName), 1),
                              _createVNode(_component_q_space),
                              (!_ctx.pending)
                                ? (_openBlock(), _createBlock(_component_q_badge, {
                                    key: 0,
                                    outline: "",
                                    color: _ctx.status === 'executed' || _ctx.status === 'approved' ? 'positive' : _ctx.status === 'rejected' || _ctx.status === 'failed' || _ctx.status === 'conflict' ? 'negative' : 'grey',
                                    "data-assist-card-status": ""
                                  }, {
                                    default: _withCtx(() => [
                                      _createTextVNode(_toDisplayString(_ctx.statusLabel), 1)
                                    ]),
                                    _: 1
                                  }, 8, ["color"]))
                                : _createCommentVNode("", true)
                            ]),
                            (_ctx.card.record && _ctx.card.record.label)
                              ? (_openBlock(), _createElementBlock("div", {
                                  key: 0,
                                  class: "q-mt-xs"
                                }, [
                                  _createVNode(_component_q_chip, {
                                    dense: "",
                                    square: "",
                                    size: "sm",
                                    icon: "sym_o_link"
                                  }, {
                                    default: _withCtx(() => [
                                      _createTextVNode(_toDisplayString(_ctx.card.record.label), 1)
                                    ]),
                                    _: 1
                                  })
                                ]))
                              : _createCommentVNode("", true),
                            (_ctx.card.risk && _ctx.card.risk.risk)
                              ? (_openBlock(), _createElementBlock("div", {
                                  key: 1,
                                  class: _normalizeClass(["text-caption q-mt-xs", _ctx.card.risk.risk === 'irreversible' ? 'text-negative' : 'm-muted']),
                                  "data-assist-risk": ""
                                }, _toDisplayString(_ctx.labels.risk) + ": " + _toDisplayString(_ctx.card.risk.risk) + " (" + _toDisplayString(_ctx.labels.riskFrom) + " " + _toDisplayString(_ctx.card.risk.skill) + ")", 3))
                              : _createCommentVNode("", true),
                            ((_ctx.card.changes || []).length && (_ctx.pending || _ctx.status === 'conflict'))
                              ? (_openBlock(), _createElementBlock("table", {
                                  key: 2,
                                  class: "m-assist-changes q-mt-xs"
                                }, [
                                  _createElementVNode("tbody", null, [
                                    (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.card.changes, (change) => {
                                      return (_openBlock(), _createElementBlock("tr", {
                                        key: change.field,
                                        "data-field": change.field
                                      }, [
                                        _createElementVNode("td", { class: "text-weight-medium" }, _toDisplayString(change.label || change.field), 1),
                                        _createElementVNode("td", null, [
                                          _createElementVNode("span", { class: "m-assist-current" }, _toDisplayString(change.current === null || change.current === undefined || change.current === '' ? '—' : change.current), 1),
                                          _createTextVNode(" → "),
                                          _createElementVNode("span", { class: "m-assist-proposed" }, _toDisplayString(change.proposed), 1)
                                        ])
                                      ], 8, ["data-field"]))
                                    }), 128))
                                  ])
                                ]))
                              : _createCommentVNode("", true),
                            (_ctx.status === 'conflict')
                              ? (_openBlock(), _createElementBlock("div", {
                                  key: 3,
                                  class: "text-caption text-negative q-mt-xs"
                                }, _toDisplayString(_ctx.card.targetName) + " " + _toDisplayString(_ctx.labels.conflict), 1))
                              : _createCommentVNode("", true),
                            (_ctx.previewRows.length)
                              ? (_openBlock(), _createElementBlock("div", {
                                  key: 4,
                                  class: "m-assist-preview text-caption q-mt-xs",
                                  "data-assist-preview": ""
                                }, [
                                  (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.previewRows, (row, index) => {
                                    return (_openBlock(), _createElementBlock("div", { key: index }, [
                                      _createTextVNode(_toDisplayString(row.action) + " " + _toDisplayString(row.entity) + " " + _toDisplayString(row.key) + ": ", 1),
                                      (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(row.fields || [], (field) => {
                                        return (_openBlock(), _createElementBlock("span", {
                                          key: field.field
                                        }, _toDisplayString(field.field) + " " + _toDisplayString(field.before) + " → " + _toDisplayString(field.after) + "; ", 1))
                                      }), 128))
                                    ]))
                                  }), 128))
                                ]))
                              : _createCommentVNode("", true),
                            (_ctx.card.preview && _ctx.card.preview.ok === false)
                              ? (_openBlock(), _createElementBlock("div", {
                                  key: 5,
                                  class: "text-caption text-negative q-mt-xs"
                                }, _toDisplayString(_ctx.card.preview.error || _ctx.card.preview.message), 1))
                              : _createCommentVNode("", true),
                            (_ctx.editing)
                              ? (_openBlock(), _createElementBlock("div", {
                                  key: 6,
                                  class: "q-mt-xs"
                                }, [
                                  _createVNode(_component_q_input, {
                                    modelValue: _ctx.argsText,
                                    "onUpdate:modelValue": $event => ((_ctx.argsText) = $event),
                                    type: "textarea",
                                    autogrow: "",
                                    dense: "",
                                    outlined: "",
                                    label: _ctx.labels.arguments,
                                    error: !!_ctx.argsError,
                                    "error-message": _ctx.argsError,
                                    "data-assist-args": ""
                                  }, null, 8, ["modelValue", "onUpdate:modelValue", "label", "error", "error-message"])
                                ]))
                              : _createCommentVNode("", true),
                            (_ctx.rejecting)
                              ? (_openBlock(), _createElementBlock("div", {
                                  key: 7,
                                  class: "q-mt-xs"
                                }, [
                                  _createVNode(_component_q_input, {
                                    modelValue: _ctx.feedback,
                                    "onUpdate:modelValue": $event => ((_ctx.feedback) = $event),
                                    type: "textarea",
                                    autogrow: "",
                                    dense: "",
                                    outlined: "",
                                    autofocus: "",
                                    label: _ctx.labels.feedback,
                                    "data-assist-feedback": ""
                                  }, null, 8, ["modelValue", "onUpdate:modelValue", "label"]),
                                  ((_ctx.card.alternatives || []).length)
                                    ? (_openBlock(), _createElementBlock("div", {
                                        key: 0,
                                        class: "q-mt-xs"
                                      }, [
                                        (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.card.alternatives, (alt) => {
                                          return (_openBlock(), _createBlock(_component_q_chip, {
                                            key: alt,
                                            dense: "",
                                            clickable: "",
                                            size: "sm",
                                            onClick: $event => (_ctx.useAlternative(alt))
                                          }, {
                                            default: _withCtx(() => [
                                              _createTextVNode(_toDisplayString(alt), 1)
                                            ]),
                                            _: 2
                                          }, 1032, ["onClick"]))
                                        }), 128))
                                      ]))
                                    : _createCommentVNode("", true)
                                ]))
                              : _createCommentVNode("", true)
                          ]),
                          _: 1
                        }),
                        (_ctx.pending)
                          ? (_openBlock(), _createBlock(_component_q_card_actions, {
                              key: 0,
                              align: "left",
                              class: "q-pt-none q-px-sm q-pb-sm q-gutter-xs"
                            }, {
                              default: _withCtx(() => [
                                (!_ctx.rejecting)
                                  ? (_openBlock(), _createElementBlock(_Fragment, { key: 0 }, [
                                      _createVNode(_component_q_btn, {
                                        unelevated: "",
                                        "no-caps": "",
                                        dense: "",
                                        color: "primary",
                                        label: _ctx.editing ? _ctx.labels.approveEdited : _ctx.labels.approve,
                                        loading: _ctx.busy,
                                        "data-assist-action": "approve",
                                        onClick: _ctx.approve
                                      }, null, 8, ["label", "loading", "onClick"]),
                                      (_ctx.card.canEdit)
                                        ? (_openBlock(), _createBlock(_component_q_btn, {
                                            key: 0,
                                            unelevated: "",
                                            "no-caps": "",
                                            dense: "",
                                            class: "m-btn-tonal",
                                            label: _ctx.labels.editInScreen,
                                            disable: _ctx.busy,
                                            "data-assist-action": "edit",
                                            onClick: _ctx.edit
                                          }, null, 8, ["label", "disable", "onClick"]))
                                        : _createCommentVNode("", true),
                                      (_ctx.editableArgs && !_ctx.editing)
                                        ? (_openBlock(), _createBlock(_component_q_btn, {
                                            key: 1,
                                            flat: "",
                                            "no-caps": "",
                                            dense: "",
                                            label: _ctx.labels.editArguments,
                                            "data-assist-action": "edit-arguments",
                                            onClick: _ctx.startEdit
                                          }, null, 8, ["label", "onClick"]))
                                        : _createCommentVNode("", true),
                                      _createVNode(_component_q_btn, {
                                        flat: "",
                                        "no-caps": "",
                                        dense: "",
                                        color: "negative",
                                        label: _ctx.labels.reject,
                                        disable: _ctx.busy,
                                        "data-assist-action": "reject",
                                        onClick: _ctx.startReject
                                      }, null, 8, ["label", "disable", "onClick"]),
                                      (_ctx.canPreview)
                                        ? (_openBlock(), _createBlock(_component_q_btn, {
                                            key: 2,
                                            flat: "",
                                            "no-caps": "",
                                            dense: "",
                                            icon: "sym_o_science",
                                            label: _ctx.labels.preview,
                                            loading: _ctx.previewing,
                                            "data-assist-action": "preview",
                                            onClick: _ctx.preview
                                          }, {
                                            default: _withCtx(() => [
                                              _createVNode(_component_q_tooltip, null, {
                                                default: _withCtx(() => [
                                                  _createTextVNode(_toDisplayString(_ctx.labels.previewNote), 1)
                                                ]),
                                                _: 1
                                              })
                                            ]),
                                            _: 1
                                          }, 8, ["label", "loading", "onClick"]))
                                        : _createCommentVNode("", true)
                                    ], 64))
                                  : (_openBlock(), _createBlock(_component_q_btn, {
                                      key: 1,
                                      unelevated: "",
                                      "no-caps": "",
                                      dense: "",
                                      color: "negative",
                                      label: _ctx.labels.rejectWith,
                                      loading: _ctx.busy,
                                      "data-assist-action": "reject-send",
                                      onClick: _ctx.reject
                                    }, null, 8, ["label", "loading", "onClick"]))
                              ]),
                              _: 1
                            }))
                          : (_ctx.status === 'conflict')
                            ? (_openBlock(), _createBlock(_component_q_card_actions, {
                                key: 1,
                                class: "q-pt-none q-px-sm q-pb-sm"
                              }, {
                                default: _withCtx(() => [
                                  _createVNode(_component_q_btn, {
                                    flat: "",
                                    "no-caps": "",
                                    dense: "",
                                    label: _ctx.labels.reviewAgain,
                                    "data-assist-action": "review",
                                    onClick: _ctx.review
                                  }, null, 8, ["label", "onClick"])
                                ]),
                                _: 1
                              }))
                            : _createCommentVNode("", true)
                      ], 64))
                    : (_ctx.kind === 'question')
                      ? (_openBlock(), _createElementBlock(_Fragment, { key: 1 }, [
                          _createVNode(_component_q_card_section, { class: "q-pa-sm" }, {
                            default: _withCtx(() => [
                              _createElementVNode("div", { class: "m-assist-pre" }, _toDisplayString(_ctx.card.text), 1),
                              (_ctx.options.length)
                                ? (_openBlock(), _createElementBlock("div", {
                                    key: 0,
                                    class: "q-mt-xs q-gutter-xs"
                                  }, [
                                    (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.options, (option) => {
                                      return (_openBlock(), _createBlock(_component_q_btn, {
                                        key: option.label,
                                        "no-caps": "",
                                        dense: "",
                                        unelevated: option.recommended || _ctx.picked.indexOf(option.label) >= 0,
                                        outline: !option.recommended && _ctx.picked.indexOf(option.label) < 0,
                                        color: "primary",
                                        label: option.label,
                                        disable: _ctx.busy,
                                        "data-assist-option": option.label,
                                        onClick: $event => (_ctx.choose(option))
                                      }, {
                                        default: _withCtx(() => [
                                          (option.recommended)
                                            ? (_openBlock(), _createBlock(_component_q_badge, {
                                                key: 0,
                                                floating: "",
                                                color: "positive",
                                                rounded: ""
                                              }, {
                                                default: _withCtx(() => [
                                                  _createTextVNode("★")
                                                ]),
                                                _: 1
                                              }))
                                            : _createCommentVNode("", true)
                                        ]),
                                        _: 2
                                      }, 1032, ["unelevated", "outline", "label", "disable", "data-assist-option", "onClick"]))
                                    }), 128))
                                  ]))
                                : _createCommentVNode("", true),
                              (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.fields, (field) => {
                                return (_openBlock(), _createElementBlock("div", {
                                  key: field.name,
                                  class: "q-mt-xs"
                                }, [
                                  (field.options && field.options.length)
                                    ? (_openBlock(), _createBlock(_component_q_select, {
                                        key: 0,
                                        modelValue: _ctx.values[field.name],
                                        "onUpdate:modelValue": $event => ((_ctx.values[field.name]) = $event),
                                        options: field.options,
                                        "emit-value": "",
                                        "map-options": "",
                                        dense: "",
                                        outlined: "",
                                        "stack-label": "",
                                        label: field.label + (field.required ? ' *' : ''),
                                        "data-assist-field": field.name
                                      }, null, 8, ["modelValue", "onUpdate:modelValue", "options", "label", "data-assist-field"]))
                                    : (field.type === 'boolean')
                                      ? (_openBlock(), _createBlock(_component_q_checkbox, {
                                          key: 1,
                                          modelValue: _ctx.values[field.name],
                                          "onUpdate:modelValue": $event => ((_ctx.values[field.name]) = $event),
                                          dense: "",
                                          label: field.label,
                                          "data-assist-field": field.name
                                        }, null, 8, ["modelValue", "onUpdate:modelValue", "label", "data-assist-field"]))
                                      : (_openBlock(), _createBlock(_component_q_input, {
                                          key: 2,
                                          modelValue: _ctx.values[field.name],
                                          "onUpdate:modelValue": $event => ((_ctx.values[field.name]) = $event),
                                          dense: "",
                                          outlined: "",
                                          "stack-label": "",
                                          type: field.type === 'number' ? 'number' : field.type === 'date' ? 'date' : 'text',
                                          label: field.label + (field.required ? ' *' : ''),
                                          "data-assist-field": field.name
                                        }, null, 8, ["modelValue", "onUpdate:modelValue", "type", "label", "data-assist-field"]))
                                ]))
                              }), 128)),
                              (_ctx.card.freeText !== false && !_ctx.budget)
                                ? (_openBlock(), _createBlock(_component_q_input, {
                                    key: 1,
                                    modelValue: _ctx.free,
                                    "onUpdate:modelValue": $event => ((_ctx.free) = $event),
                                    dense: "",
                                    outlined: "",
                                    class: "q-mt-xs",
                                    label: _ctx.options.length ? _ctx.labels.other : _ctx.labels.answer,
                                    "data-assist-free": "",
                                    onKeyup: _withKeys(_ctx.submitAnswer, ["enter"])
                                  }, null, 8, ["modelValue", "onUpdate:modelValue", "label", "onKeyup"]))
                                : _createCommentVNode("", true)
                            ]),
                            _: 1
                          }),
                          (!_ctx.budget && (_ctx.fields.length || _ctx.card.multi || _ctx.card.freeText !== false))
                            ? (_openBlock(), _createBlock(_component_q_card_actions, {
                                key: 0,
                                class: "q-pt-none q-px-sm q-pb-sm"
                              }, {
                                default: _withCtx(() => [
                                  _createVNode(_component_q_btn, {
                                    unelevated: "",
                                    "no-caps": "",
                                    dense: "",
                                    color: "primary",
                                    label: _ctx.labels.answer,
                                    loading: _ctx.busy,
                                    "data-assist-action": "answer",
                                    onClick: _ctx.submitAnswer
                                  }, null, 8, ["label", "loading", "onClick"])
                                ]),
                                _: 1
                              }))
                            : _createCommentVNode("", true)
                        ], 64))
                      : (_ctx.kind === 'handoff')
                        ? (_openBlock(), _createElementBlock(_Fragment, { key: 2 }, [
                            _createVNode(_component_q_card_section, {
                              class: "q-pa-sm",
                              id: "m-assist-handoff"
                            }, {
                              default: _withCtx(() => [
                                _createElementVNode("div", { class: "text-weight-medium" }, _toDisplayString(_ctx.card.lastOk === false || _ctx.status === 'failed' ? _ctx.labels.handoffFailed : _ctx.labels.handoff), 1),
                                (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.handoffValues, (value) => {
                                  return (_openBlock(), _createElementBlock("div", {
                                    key: value.field,
                                    class: "text-caption"
                                  }, [
                                    _createTextVNode(_toDisplayString(value.label || value.field) + ": ", 1),
                                    _createElementVNode("span", { class: "m-assist-proposed" }, _toDisplayString(value.value), 1)
                                  ]))
                                }), 128))
                              ]),
                              _: 1
                            }),
                            _createVNode(_component_q_card_actions, { class: "q-pt-none q-px-sm q-pb-sm" }, {
                              default: _withCtx(() => [
                                _createVNode(_component_q_btn, {
                                  flat: "",
                                  "no-caps": "",
                                  dense: "",
                                  label: _ctx.labels.skip,
                                  loading: _ctx.status === 'abandoning',
                                  "data-assist-action": "skip",
                                  onClick: _ctx.skip
                                }, null, 8, ["label", "loading", "onClick"])
                              ]),
                              _: 1
                            })
                          ], 64))
                        : _createCommentVNode("", true)
                ]),
                _: 1
              }, 8, ["data-assist-card", "data-status", "data-approval-id", "data-question-id", "data-handoff-id"]))
            }
        } },
        "m-assist-composer": { hash: "dc3df590", make: function (Vue) {
            const { toDisplayString: _toDisplayString, openBlock: _openBlock, createElementBlock: _createElementBlock, createCommentVNode: _createCommentVNode, renderList: _renderList, Fragment: _Fragment, createTextVNode: _createTextVNode, resolveComponent: _resolveComponent, withCtx: _withCtx, createBlock: _createBlock, createElementVNode: _createElementVNode, createVNode: _createVNode } = Vue

            return function render(_ctx, _cache) {
              const _component_q_chip = _resolveComponent("q-chip")
              const _component_q_spinner = _resolveComponent("q-spinner")
              const _component_q_item_label = _resolveComponent("q-item-label")
              const _component_q_item_section = _resolveComponent("q-item-section")
              const _component_q_item = _resolveComponent("q-item")
              const _component_q_list = _resolveComponent("q-list")
              const _component_q_tooltip = _resolveComponent("q-tooltip")
              const _component_q_btn = _resolveComponent("q-btn")
              const _component_q_input = _resolveComponent("q-input")

              return (_openBlock(), _createElementBlock("div", {
                class: "m-assist-composer q-pa-sm",
                id: "m-assist-composer",
                onDragover: _ctx.onDragOver,
                onDrop: _ctx.onDrop,
                onPaste: _ctx.onPaste
              }, [
                (_ctx.s.error)
                  ? (_openBlock(), _createElementBlock("div", {
                      key: 0,
                      class: "text-caption text-negative q-mb-xs",
                      id: "m-assist-error"
                    }, _toDisplayString(_ctx.s.error), 1))
                  : _createCommentVNode("", true),
                (_ctx.prompts.length)
                  ? (_openBlock(), _createElementBlock("div", {
                      key: 1,
                      class: "row q-gutter-xs q-mb-xs m-assist-prompts",
                      id: "m-assist-prompts"
                    }, [
                      (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.prompts, (prompt, index) => {
                        return (_openBlock(), _createBlock(_component_q_chip, {
                          key: prompt.promptId || index,
                          clickable: "",
                          dense: "",
                          size: "sm",
                          icon: prompt.send ? 'sym_o_send' : undefined,
                          "data-assist-prompt": "",
                          "data-send": prompt.send ? 'true' : 'false',
                          onClick: $event => (_ctx.choosePrompt(prompt))
                        }, {
                          default: _withCtx(() => [
                            _createTextVNode(_toDisplayString(prompt.label || prompt.text), 1)
                          ]),
                          _: 2
                        }, 1032, ["icon", "data-send", "onClick"]))
                      }), 128))
                    ]))
                  : _createCommentVNode("", true),
                (_ctx.s.attachments.length)
                  ? (_openBlock(), _createElementBlock("div", {
                      key: 2,
                      class: "row q-gutter-xs q-mb-xs",
                      id: "m-assist-attachments"
                    }, [
                      (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.s.attachments, (file) => {
                        return (_openBlock(), _createBlock(_component_q_chip, {
                          key: file.key,
                          dense: "",
                          square: "",
                          removable: "",
                          size: "sm",
                          icon: file.status === 'failed' ? 'sym_o_error' : 'sym_o_attach_file',
                          color: file.status === 'failed' ? 'negative' : undefined,
                          "text-color": file.status === 'failed' ? 'white' : undefined,
                          title: file.error || file.mediaType,
                          "data-assist-attachment": file.status,
                          "remove-aria-label": _ctx.labels.remove,
                          onRemove: $event => (_ctx.removeFile(file))
                        }, {
                          default: _withCtx(() => [
                            (file.status === 'uploading')
                              ? (_openBlock(), _createBlock(_component_q_spinner, {
                                  key: 0,
                                  size: "12px",
                                  class: "q-mr-xs",
                                  "aria-label": _ctx.labels.uploading
                                }, null, 8, ["aria-label"]))
                              : _createCommentVNode("", true),
                            _createTextVNode(_toDisplayString(file.fileName || file.name), 1),
                            _createElementVNode("span", { class: "q-ml-xs m-muted" }, _toDisplayString(_ctx.sizeText(file.size)), 1)
                          ]),
                          _: 2
                        }, 1032, ["icon", "color", "text-color", "title", "data-assist-attachment", "remove-aria-label", "onRemove"]))
                      }), 128))
                    ]))
                  : _createCommentVNode("", true),
                (_ctx.commandHint)
                  ? (_openBlock(), _createElementBlock("div", {
                      key: 3,
                      class: "text-caption m-muted q-mb-xs"
                    }, _toDisplayString(_ctx.commandHint), 1))
                  : _createCommentVNode("", true),
                (_ctx.mention && _ctx.mentionRows.length)
                  ? (_openBlock(), _createBlock(_component_q_list, {
                      key: 4,
                      dense: "",
                      bordered: "",
                      class: "m-assist-mentions q-mb-xs",
                      id: "m-assist-mentions"
                    }, {
                      default: _withCtx(() => [
                        (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.mentionRows, (row, index) => {
                          return (_openBlock(), _createBlock(_component_q_item, {
                            key: row.key || (row.recordEntity + row.recordId),
                            clickable: "",
                            dense: "",
                            active: index === _ctx.selected,
                            onClick: $event => (_ctx.pick(row))
                          }, {
                            default: _withCtx(() => [
                              _createVNode(_component_q_item_section, null, {
                                default: _withCtx(() => [
                                  _createVNode(_component_q_item_label, { lines: "1" }, {
                                    default: _withCtx(() => [
                                      _createTextVNode(_toDisplayString(row.title), 1)
                                    ]),
                                    _: 2
                                  }, 1024),
                                  (row.subtitle)
                                    ? (_openBlock(), _createBlock(_component_q_item_label, {
                                        key: 0,
                                        caption: "",
                                        lines: "1"
                                      }, {
                                        default: _withCtx(() => [
                                          _createTextVNode(_toDisplayString(row.subtitle), 1)
                                        ]),
                                        _: 2
                                      }, 1024))
                                    : _createCommentVNode("", true)
                                ]),
                                _: 2
                              }, 1024)
                            ]),
                            _: 2
                          }, 1032, ["active", "onClick"]))
                        }), 128))
                      ]),
                      _: 1
                    }))
                  : _createCommentVNode("", true),
                _createElementVNode("div", { class: "row items-end no-wrap q-gutter-x-xs" }, [
                  (_ctx.attachOn)
                    ? (_openBlock(), _createElementBlock(_Fragment, { key: 0 }, [
                        _createVNode(_component_q_btn, {
                          flat: "",
                          round: "",
                          dense: "",
                          icon: "sym_o_attach_file",
                          id: "m-assist-attach",
                          "aria-label": _ctx.labels.attach,
                          onClick: _ctx.pickFiles
                        }, {
                          default: _withCtx(() => [
                            _createVNode(_component_q_tooltip, null, {
                              default: _withCtx(() => [
                                _createTextVNode(_toDisplayString(_ctx.labels.attach), 1)
                              ]),
                              _: 1
                            })
                          ]),
                          _: 1
                        }, 8, ["aria-label", "onClick"]),
                        _createElementVNode("input", {
                          ref: "files",
                          type: "file",
                          multiple: "",
                          accept: _ctx.accept,
                          id: "m-assist-file",
                          style: {"display":"none"},
                          onChange: _ctx.onFiles
                        }, null, 40, ["accept", "onChange"])
                      ], 64))
                    : _createCommentVNode("", true),
                  (_ctx.dictation)
                    ? (_openBlock(), _createBlock(_component_q_btn, {
                        key: 1,
                        flat: "",
                        round: "",
                        dense: "",
                        icon: _ctx.s.dictation.recording ? 'sym_o_stop_circle' : 'sym_o_mic',
                        color: _ctx.s.dictation.recording ? 'negative' : undefined,
                        loading: _ctx.s.dictation.busy,
                        id: "m-assist-dictate",
                        "data-assist-dictation": _ctx.dictation,
                        "data-recording": _ctx.s.dictation.recording ? 'true' : undefined,
                        "aria-label": _ctx.s.dictation.recording ? _ctx.labels.stopDictation : _ctx.labels.dictate,
                        onClick: _ctx.toggleDictation
                      }, {
                        default: _withCtx(() => [
                          _createVNode(_component_q_tooltip, null, {
                            default: _withCtx(() => [
                              _createTextVNode(_toDisplayString(_ctx.s.dictation.recording ? _ctx.labels.stopDictation : _ctx.labels.dictate), 1)
                            ]),
                            _: 1
                          })
                        ]),
                        _: 1
                      }, 8, ["icon", "color", "loading", "data-assist-dictation", "data-recording", "aria-label", "onClick"]))
                    : _createCommentVNode("", true),
                  _createVNode(_component_q_input, {
                    modelValue: _ctx.draft,
                    "onUpdate:modelValue": [$event => ((_ctx.draft) = $event), _ctx.onInput],
                    type: "textarea",
                    autogrow: "",
                    dense: "",
                    outlined: "",
                    class: "col m-assist-input",
                    "input-class": "m-assist-textarea",
                    placeholder: _ctx.labels.placeholder,
                    for: "m-assist-input",
                    name: "assistText",
                    onKeydown: _ctx.onKey
                  }, null, 8, ["modelValue", "onUpdate:modelValue", "placeholder", "onKeydown"]),
                  (_ctx.running)
                    ? (_openBlock(), _createBlock(_component_q_btn, {
                        key: 2,
                        unelevated: "",
                        round: "",
                        dense: "",
                        color: "negative",
                        icon: "sym_o_stop",
                        id: "m-assist-stop",
                        "aria-label": _ctx.labels.stop,
                        onClick: _ctx.stop
                      }, {
                        default: _withCtx(() => [
                          _createVNode(_component_q_tooltip, null, {
                            default: _withCtx(() => [
                              _createTextVNode(_toDisplayString(_ctx.labels.stop), 1)
                            ]),
                            _: 1
                          })
                        ]),
                        _: 1
                      }, 8, ["aria-label", "onClick"]))
                    : _createCommentVNode("", true),
                  _createVNode(_component_q_btn, {
                    unelevated: "",
                    round: "",
                    dense: "",
                    color: "primary",
                    icon: "sym_o_send",
                    id: "m-assist-send",
                    "aria-label": _ctx.labels.send,
                    disable: !_ctx.draft.trim() || _ctx.uploading,
                    onClick: _ctx.send
                  }, null, 8, ["aria-label", "disable", "onClick"])
                ])
              ], 40, ["onDragover", "onDrop", "onPaste"]))
            }
        } },
        "m-assist-history": { hash: "6633506c", make: function (Vue) {
            const { resolveComponent: _resolveComponent, createVNode: _createVNode, withCtx: _withCtx, createElementVNode: _createElementVNode, toDisplayString: _toDisplayString, createTextVNode: _createTextVNode, openBlock: _openBlock, createBlock: _createBlock, createCommentVNode: _createCommentVNode, renderList: _renderList, Fragment: _Fragment, createElementBlock: _createElementBlock } = Vue

            return function render(_ctx, _cache) {
              const _component_q_icon = _resolveComponent("q-icon")
              const _component_q_input = _resolveComponent("q-input")
              const _component_q_item_section = _resolveComponent("q-item-section")
              const _component_q_item = _resolveComponent("q-item")
              const _component_q_item_label = _resolveComponent("q-item-label")
              const _component_q_spinner = _resolveComponent("q-spinner")
              const _component_q_list = _resolveComponent("q-list")

              return (_openBlock(), _createElementBlock("div", {
                class: "m-assist-history column no-wrap",
                id: "m-assist-history-pane"
              }, [
                _createElementVNode("div", { class: "q-pa-sm" }, [
                  _createVNode(_component_q_input, {
                    modelValue: _ctx.history.search,
                    "onUpdate:modelValue": [$event => ((_ctx.history.search) = $event), _ctx.search],
                    dense: "",
                    outlined: "",
                    clearable: "",
                    placeholder: _ctx.labels.searchConversations
                  }, {
                    prepend: _withCtx(() => [
                      _createVNode(_component_q_icon, {
                        name: "sym_o_search",
                        size: "xs"
                      })
                    ]),
                    _: 1
                  }, 8, ["modelValue", "onUpdate:modelValue", "placeholder"])
                ]),
                _createVNode(_component_q_list, {
                  class: "col scroll",
                  separator: "",
                  dense: ""
                }, {
                  default: _withCtx(() => [
                    (!_ctx.history.loading && !_ctx.history.list.length)
                      ? (_openBlock(), _createBlock(_component_q_item, { key: 0 }, {
                          default: _withCtx(() => [
                            _createVNode(_component_q_item_section, { class: "m-muted" }, {
                              default: _withCtx(() => [
                                _createTextVNode(_toDisplayString(_ctx.labels.noConversations), 1)
                              ]),
                              _: 1
                            })
                          ]),
                          _: 1
                        }))
                      : _createCommentVNode("", true),
                    (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.history.list, (item) => {
                      return (_openBlock(), _createBlock(_component_q_item, {
                        key: item.conversationId,
                        clickable: "",
                        "data-conversation-id": item.conversationId,
                        onClick: $event => (_ctx.openOne(item))
                      }, {
                        default: _withCtx(() => [
                          _createVNode(_component_q_item_section, null, {
                            default: _withCtx(() => [
                              _createVNode(_component_q_item_label, { lines: "1" }, {
                                default: _withCtx(() => [
                                  _createTextVNode(_toDisplayString(item.title || item.conversationId), 1)
                                ]),
                                _: 2
                              }, 1024),
                              _createVNode(_component_q_item_label, { caption: "" }, {
                                default: _withCtx(() => [
                                  _createTextVNode(_toDisplayString(_ctx.when(item.lastMessageDate || item.createdDate)), 1)
                                ]),
                                _: 2
                              }, 1024)
                            ]),
                            _: 2
                          }, 1024),
                          _createVNode(_component_q_item_section, { side: "" }, {
                            default: _withCtx(() => [
                              (item.waiting)
                                ? (_openBlock(), _createBlock(_component_q_icon, {
                                    key: 0,
                                    name: "sym_o_pending_actions",
                                    color: "warning",
                                    size: "xs"
                                  }))
                                : (item.activeRunId)
                                  ? (_openBlock(), _createBlock(_component_q_spinner, {
                                      key: 1,
                                      size: "14px"
                                    }))
                                  : _createCommentVNode("", true)
                            ]),
                            _: 2
                          }, 1024)
                        ]),
                        _: 2
                      }, 1032, ["data-conversation-id", "onClick"]))
                    }), 128))
                  ]),
                  _: 1
                })
              ]))
            }
        } },
        "m-assist-markdown": { hash: "c4bef455", make: function (Vue) {
            const { toDisplayString: _toDisplayString, openBlock: _openBlock, createElementBlock: _createElementBlock, createCommentVNode: _createCommentVNode } = Vue

            return function render(_ctx, _cache) {
              return (_openBlock(), _createElementBlock("div", {
                class: "m-assist-markdown m-markdown",
                onClick: _ctx.onClick
              }, [
                (_ctx.failed)
                  ? (_openBlock(), _createElementBlock("div", {
                      key: 0,
                      class: "m-assist-pre"
                    }, _toDisplayString(_ctx.text), 1))
                  : (_openBlock(), _createElementBlock("div", {
                      key: 1,
                      class: "m-markdown-body",
                      innerHTML: _ctx.html
                    }, null, 8, ["innerHTML"]))
              ], 8, ["onClick"]))
            }
        } },
        "m-assist-memory": { hash: "fb3f308d", make: function (Vue) {
            const { toDisplayString: _toDisplayString, createElementVNode: _createElementVNode, resolveComponent: _resolveComponent, createVNode: _createVNode, openBlock: _openBlock, createElementBlock: _createElementBlock, createCommentVNode: _createCommentVNode, withModifiers: _withModifiers, withKeys: _withKeys, createTextVNode: _createTextVNode, withCtx: _withCtx, createBlock: _createBlock, renderList: _renderList, Fragment: _Fragment } = Vue

            return function render(_ctx, _cache) {
              const _component_q_toggle = _resolveComponent("q-toggle")
              const _component_q_checkbox = _resolveComponent("q-checkbox")
              const _component_q_input = _resolveComponent("q-input")
              const _component_q_btn = _resolveComponent("q-btn")
              const _component_q_item_section = _resolveComponent("q-item-section")
              const _component_q_item = _resolveComponent("q-item")
              const _component_q_space = _resolveComponent("q-space")
              const _component_q_item_label = _resolveComponent("q-item-label")
              const _component_q_tooltip = _resolveComponent("q-tooltip")
              const _component_q_list = _resolveComponent("q-list")

              return (_openBlock(), _createElementBlock("div", {
                class: "m-assist-memory scroll q-pa-sm",
                id: "m-assist-memory-pane"
              }, [
                _createElementVNode("div", { class: "text-subtitle2 q-mb-xs" }, _toDisplayString(_ctx.labels.memory), 1),
                _createElementVNode("div", null, [
                  _createVNode(_component_q_toggle, {
                    dense: "",
                    "model-value": _ctx.gate.effective,
                    label: _ctx.labels.remember,
                    disable: _ctx.busy,
                    id: "m-assist-memory-create",
                    "data-assist-memory": "create",
                    "onUpdate:modelValue": _ctx.setCreate
                  }, null, 8, ["model-value", "label", "disable", "onUpdate:modelValue"])
                ]),
                _createElementVNode("div", { class: "q-mt-xs" }, [
                  _createVNode(_component_q_checkbox, {
                    dense: "",
                    size: "sm",
                    "model-value": _ctx.onlyHere,
                    label: _ctx.labels.onlyConversation,
                    disable: _ctx.busy || !_ctx.conversationId,
                    id: "m-assist-memory-scope",
                    "data-assist-memory": "scope",
                    "onUpdate:modelValue": _ctx.setOnlyHere
                  }, null, 8, ["model-value", "label", "disable", "onUpdate:modelValue"])
                ]),
                _createElementVNode("div", { class: "text-caption m-muted q-my-xs" }, _toDisplayString(_ctx.labels.memoryNote), 1),
                (_ctx.pane.error)
                  ? (_openBlock(), _createElementBlock("div", {
                      key: 0,
                      class: "text-caption text-negative q-mb-xs m-assist-pre",
                      "data-assist-memory-error": ""
                    }, _toDisplayString(_ctx.pane.error), 1))
                  : _createCommentVNode("", true),
                _createElementVNode("div", { class: "row no-wrap items-start q-gutter-x-xs q-mb-sm" }, [
                  _createVNode(_component_q_input, {
                    modelValue: _ctx.adding,
                    "onUpdate:modelValue": $event => ((_ctx.adding) = $event),
                    dense: "",
                    outlined: "",
                    class: "col",
                    maxlength: "200",
                    placeholder: _ctx.labels.memoryText,
                    for: "m-assist-memory-add",
                    onKeydown: _withKeys(_withModifiers(_ctx.add, ["prevent"]), ["enter"])
                  }, null, 8, ["modelValue", "onUpdate:modelValue", "placeholder", "onKeydown"]),
                  _createVNode(_component_q_input, {
                    modelValue: _ctx.addDays,
                    "onUpdate:modelValue": $event => ((_ctx.addDays) = $event),
                    dense: "",
                    outlined: "",
                    type: "number",
                    min: "1",
                    class: "m-assist-days",
                    placeholder: _ctx.labels.memoryDays,
                    "aria-label": _ctx.labels.memoryDays
                  }, null, 8, ["modelValue", "onUpdate:modelValue", "placeholder", "aria-label"]),
                  _createVNode(_component_q_btn, {
                    unelevated: "",
                    dense: "",
                    "no-caps": "",
                    color: "primary",
                    label: _ctx.labels.addMemory,
                    disable: !_ctx.adding.trim() || _ctx.busy,
                    "data-assist-action": "memory-add",
                    onClick: _ctx.add
                  }, null, 8, ["label", "disable", "onClick"])
                ]),
                _createVNode(_component_q_list, {
                  dense: "",
                  separator: "",
                  bordered: "",
                  class: "q-mb-md"
                }, {
                  default: _withCtx(() => [
                    (!_ctx.pane.loading && !_ctx.pane.memories.length)
                      ? (_openBlock(), _createBlock(_component_q_item, { key: 0 }, {
                          default: _withCtx(() => [
                            _createVNode(_component_q_item_section, { class: "m-muted" }, {
                              default: _withCtx(() => [
                                _createTextVNode(_toDisplayString(_ctx.labels.noMemories), 1)
                              ]),
                              _: 1
                            })
                          ]),
                          _: 1
                        }))
                      : _createCommentVNode("", true),
                    (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.pane.memories, (memory) => {
                      return (_openBlock(), _createBlock(_component_q_item, {
                        key: memory.memoryId,
                        "data-assist-memory-id": memory.memoryId
                      }, {
                        default: _withCtx(() => [
                          (_ctx.editing === memory.memoryId)
                            ? (_openBlock(), _createBlock(_component_q_item_section, { key: 0 }, {
                                default: _withCtx(() => [
                                  _createVNode(_component_q_input, {
                                    modelValue: _ctx.editText,
                                    "onUpdate:modelValue": $event => ((_ctx.editText) = $event),
                                    dense: "",
                                    outlined: "",
                                    autogrow: "",
                                    maxlength: "200",
                                    "data-assist-memory-text": "",
                                    onKeydown: _withKeys(_withModifiers($event => (_ctx.saveEdit(memory)), ["prevent"]), ["enter"])
                                  }, null, 8, ["modelValue", "onUpdate:modelValue", "onKeydown"]),
                                  _createElementVNode("div", { class: "row no-wrap items-center q-gutter-xs q-mt-xs" }, [
                                    _createVNode(_component_q_input, {
                                      modelValue: _ctx.editDays,
                                      "onUpdate:modelValue": $event => ((_ctx.editDays) = $event),
                                      dense: "",
                                      outlined: "",
                                      type: "number",
                                      min: "1",
                                      class: "m-assist-days",
                                      placeholder: _ctx.labels.memoryDays,
                                      "aria-label": _ctx.labels.memoryDays
                                    }, null, 8, ["modelValue", "onUpdate:modelValue", "placeholder", "aria-label"]),
                                    _createVNode(_component_q_space),
                                    _createVNode(_component_q_btn, {
                                      flat: "",
                                      dense: "",
                                      "no-caps": "",
                                      label: _ctx.labels.cancel,
                                      onClick: $event => (_ctx.editing = null)
                                    }, null, 8, ["label", "onClick"]),
                                    _createVNode(_component_q_btn, {
                                      unelevated: "",
                                      dense: "",
                                      "no-caps": "",
                                      color: "primary",
                                      label: _ctx.labels.save,
                                      disable: !_ctx.editText.trim() || _ctx.busy,
                                      "data-assist-action": "memory-save",
                                      onClick: $event => (_ctx.saveEdit(memory))
                                    }, null, 8, ["label", "disable", "onClick"])
                                  ])
                                ]),
                                _: 2
                              }, 1024))
                            : (_openBlock(), _createElementBlock(_Fragment, { key: 1 }, [
                                _createVNode(_component_q_item_section, null, {
                                  default: _withCtx(() => [
                                    _createVNode(_component_q_item_label, { class: "m-assist-pre" }, {
                                      default: _withCtx(() => [
                                        _createTextVNode(_toDisplayString(memory.text), 1)
                                      ]),
                                      _: 2
                                    }, 1024),
                                    _createVNode(_component_q_item_label, { caption: "" }, {
                                      default: _withCtx(() => [
                                        _createTextVNode(_toDisplayString(_ctx.when(memory.createdDate)), 1),
                                        (memory.thruDate)
                                          ? (_openBlock(), _createElementBlock("span", { key: 0 }, " · " + _toDisplayString(_ctx.labels.until) + " " + _toDisplayString(_ctx.when(memory.thruDate)), 1))
                                          : _createCommentVNode("", true)
                                      ]),
                                      _: 2
                                    }, 1024)
                                  ]),
                                  _: 2
                                }, 1024),
                                _createVNode(_component_q_item_section, { side: "" }, {
                                  default: _withCtx(() => [
                                    _createElementVNode("div", { class: "row no-wrap" }, [
                                      _createVNode(_component_q_btn, {
                                        flat: "",
                                        round: "",
                                        dense: "",
                                        size: "sm",
                                        icon: "sym_o_edit",
                                        "aria-label": _ctx.labels.edit,
                                        "data-assist-action": "memory-edit",
                                        onClick: $event => (_ctx.startEdit(memory))
                                      }, null, 8, ["aria-label", "onClick"]),
                                      _createVNode(_component_q_btn, {
                                        flat: "",
                                        round: "",
                                        dense: "",
                                        size: "sm",
                                        icon: "sym_o_delete",
                                        "aria-label": _ctx.labels.forget,
                                        disable: _ctx.busy,
                                        "data-assist-action": "memory-forget",
                                        onClick: $event => (_ctx.forget(memory))
                                      }, {
                                        default: _withCtx(() => [
                                          _createVNode(_component_q_tooltip, null, {
                                            default: _withCtx(() => [
                                              _createTextVNode(_toDisplayString(_ctx.labels.forget), 1)
                                            ]),
                                            _: 1
                                          })
                                        ]),
                                        _: 1
                                      }, 8, ["aria-label", "disable", "onClick"])
                                    ])
                                  ]),
                                  _: 2
                                }, 1024)
                              ], 64))
                        ]),
                        _: 2
                      }, 1032, ["data-assist-memory-id"]))
                    }), 128))
                  ]),
                  _: 1
                }),
                _createElementVNode("div", { class: "text-subtitle2 q-mb-xs" }, _toDisplayString(_ctx.labels.lessons), 1),
                _createVNode(_component_q_list, {
                  dense: "",
                  separator: "",
                  bordered: ""
                }, {
                  default: _withCtx(() => [
                    (!_ctx.pane.loading && !_ctx.pane.lessons.length)
                      ? (_openBlock(), _createBlock(_component_q_item, { key: 0 }, {
                          default: _withCtx(() => [
                            _createVNode(_component_q_item_section, { class: "m-muted" }, {
                              default: _withCtx(() => [
                                _createTextVNode(_toDisplayString(_ctx.labels.noLessons), 1)
                              ]),
                              _: 1
                            })
                          ]),
                          _: 1
                        }))
                      : _createCommentVNode("", true),
                    (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.pane.lessons, (lesson) => {
                      return (_openBlock(), _createBlock(_component_q_item, {
                        key: lesson.lessonId,
                        "data-assist-lesson-id": lesson.lessonId,
                        "data-status": lesson.status
                      }, {
                        default: _withCtx(() => [
                          _createVNode(_component_q_item_section, null, {
                            default: _withCtx(() => [
                              _createVNode(_component_q_item_label, { class: "m-assist-pre" }, {
                                default: _withCtx(() => [
                                  _createTextVNode(_toDisplayString(lesson.text), 1)
                                ]),
                                _: 2
                              }, 1024),
                              _createVNode(_component_q_item_label, { caption: "" }, {
                                default: _withCtx(() => [
                                  _createTextVNode(_toDisplayString(lesson.anchor) + " · " + _toDisplayString(lesson.status), 1)
                                ]),
                                _: 2
                              }, 1024)
                            ]),
                            _: 2
                          }, 1024),
                          (lesson.status === 'proposed')
                            ? (_openBlock(), _createBlock(_component_q_item_section, {
                                key: 0,
                                side: ""
                              }, {
                                default: _withCtx(() => [
                                  _createVNode(_component_q_btn, {
                                    flat: "",
                                    dense: "",
                                    "no-caps": "",
                                    size: "sm",
                                    color: "negative",
                                    label: _ctx.labels.deleteLesson,
                                    disable: _ctx.busy,
                                    "data-assist-action": "lesson-delete",
                                    onClick: $event => (_ctx.deleteLesson(lesson))
                                  }, null, 8, ["label", "disable", "onClick"])
                                ]),
                                _: 2
                              }, 1024))
                            : _createCommentVNode("", true)
                        ]),
                        _: 2
                      }, 1032, ["data-assist-lesson-id", "data-status"]))
                    }), 128))
                  ]),
                  _: 1
                })
              ]))
            }
        } },
        "m-assist-skill-editor": { hash: "30559212", make: function (Vue) {
            const { resolveComponent: _resolveComponent, createVNode: _createVNode, toDisplayString: _toDisplayString, createElementVNode: _createElementVNode, createTextVNode: _createTextVNode, openBlock: _openBlock, createElementBlock: _createElementBlock, createCommentVNode: _createCommentVNode, Fragment: _Fragment } = Vue

            return function render(_ctx, _cache) {
              const _component_q_btn = _resolveComponent("q-btn")
              const _component_q_space = _resolveComponent("q-space")
              const _component_q_spinner_dots = _resolveComponent("q-spinner-dots")
              const _component_q_input = _resolveComponent("q-input")

              return (_openBlock(), _createElementBlock("div", {
                class: "m-assist-skill-editor column no-wrap q-pa-sm",
                id: "m-assist-skill-editor",
                "data-skill-id": _ctx.editor.skillId || undefined
              }, [
                _createElementVNode("div", { class: "row items-center no-wrap q-mb-xs" }, [
                  _createVNode(_component_q_btn, {
                    flat: "",
                    dense: "",
                    "no-caps": "",
                    icon: "sym_o_arrow_back",
                    label: _ctx.labels.back,
                    "data-assist-action": "skill-back",
                    onClick: _ctx.back
                  }, null, 8, ["label", "onClick"]),
                  _createVNode(_component_q_space),
                  _createElementVNode("div", { class: "text-subtitle2 ellipsis" }, _toDisplayString(_ctx.editor.title || _ctx.labels.skill), 1)
                ]),
                (_ctx.editor.loading)
                  ? (_openBlock(), _createElementBlock("div", {
                      key: 0,
                      class: "q-pa-md m-muted row items-center"
                    }, [
                      _createVNode(_component_q_spinner_dots, {
                        size: "18px",
                        class: "q-mr-xs"
                      }),
                      _createTextVNode(_toDisplayString(_ctx.editor.runId ? _ctx.labels.drafting : _ctx.labels.loading), 1)
                    ]))
                  : (_openBlock(), _createElementBlock(_Fragment, { key: 1 }, [
                      _createVNode(_component_q_input, {
                        modelValue: _ctx.editor.markdown,
                        "onUpdate:modelValue": $event => ((_ctx.editor.markdown) = $event),
                        type: "textarea",
                        outlined: "",
                        dense: "",
                        class: "m-assist-skill-text",
                        "input-class": "m-assist-skill-input",
                        for: "m-assist-skill-markdown"
                      }, null, 8, ["modelValue", "onUpdate:modelValue"]),
                      (_ctx.editor.errors)
                        ? (_openBlock(), _createElementBlock("div", {
                            key: 0,
                            class: "text-caption text-negative q-mt-xs m-assist-pre",
                            "data-assist-skill-errors": ""
                          }, _toDisplayString(_ctx.editor.errors), 1))
                        : _createCommentVNode("", true),
                      (_ctx.editor.saved)
                        ? (_openBlock(), _createElementBlock("div", {
                            key: 1,
                            class: "text-caption text-positive q-mt-xs",
                            "data-assist-skill-saved": ""
                          }, _toDisplayString(_ctx.labels.skillSaved) + ": " + _toDisplayString(_ctx.editor.saved.name) + _toDisplayString(_ctx.editor.saved.versionNum ? ' v' + _ctx.editor.saved.versionNum : ''), 1))
                        : _createCommentVNode("", true),
                      _createElementVNode("div", { class: "row q-gutter-xs q-mt-xs" }, [
                        _createVNode(_component_q_btn, {
                          unelevated: "",
                          dense: "",
                          "no-caps": "",
                          color: "primary",
                          label: _ctx.labels.save,
                          loading: _ctx.editor.saving,
                          disable: !_ctx.editor.markdown.trim(),
                          id: "m-assist-skill-save",
                          "data-assist-action": "skill-save",
                          onClick: _ctx.save
                        }, null, 8, ["label", "loading", "disable", "onClick"])
                      ])
                    ], 64))
              ], 8, ["data-skill-id"]))
            }
        } },
        "m-assist-skills": { hash: "fc1e0de7", make: function (Vue) {
            const { toDisplayString: _toDisplayString, createElementVNode: _createElementVNode, openBlock: _openBlock, createElementBlock: _createElementBlock, createCommentVNode: _createCommentVNode, createTextVNode: _createTextVNode, resolveComponent: _resolveComponent, withCtx: _withCtx, createVNode: _createVNode, createBlock: _createBlock, renderList: _renderList, Fragment: _Fragment, withModifiers: _withModifiers } = Vue

            return function render(_ctx, _cache) {
              const _component_q_item_section = _resolveComponent("q-item-section")
              const _component_q_item = _resolveComponent("q-item")
              const _component_q_item_label = _resolveComponent("q-item-label")
              const _component_q_btn = _resolveComponent("q-btn")
              const _component_q_list = _resolveComponent("q-list")

              return (_openBlock(), _createElementBlock("div", {
                class: "m-assist-skills scroll q-pa-sm",
                id: "m-assist-skills-pane"
              }, [
                _createElementVNode("div", { class: "text-subtitle2 q-mb-xs" }, _toDisplayString(_ctx.labels.mySkills), 1),
                (_ctx.pane.error)
                  ? (_openBlock(), _createElementBlock("div", {
                      key: 0,
                      class: "text-caption text-negative q-mb-xs m-assist-pre"
                    }, _toDisplayString(_ctx.pane.error), 1))
                  : _createCommentVNode("", true),
                _createVNode(_component_q_list, {
                  dense: "",
                  separator: "",
                  bordered: ""
                }, {
                  default: _withCtx(() => [
                    (!_ctx.pane.loading && !_ctx.pane.list.length)
                      ? (_openBlock(), _createBlock(_component_q_item, { key: 0 }, {
                          default: _withCtx(() => [
                            _createVNode(_component_q_item_section, { class: "m-muted" }, {
                              default: _withCtx(() => [
                                _createTextVNode(_toDisplayString(_ctx.labels.noSkills), 1)
                              ]),
                              _: 1
                            })
                          ]),
                          _: 1
                        }))
                      : _createCommentVNode("", true),
                    (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.pane.list, (skill) => {
                      return (_openBlock(), _createBlock(_component_q_item, {
                        key: skill.skillId,
                        clickable: "",
                        "data-assist-skill": skill.name,
                        "data-status": skill.status,
                        onClick: $event => (_ctx.open(skill))
                      }, {
                        default: _withCtx(() => [
                          _createVNode(_component_q_item_section, null, {
                            default: _withCtx(() => [
                              _createVNode(_component_q_item_label, { lines: "1" }, {
                                default: _withCtx(() => [
                                  _createTextVNode(_toDisplayString(skill.title || skill.name), 1)
                                ]),
                                _: 2
                              }, 1024),
                              (skill.description)
                                ? (_openBlock(), _createBlock(_component_q_item_label, {
                                    key: 0,
                                    caption: "",
                                    lines: "2"
                                  }, {
                                    default: _withCtx(() => [
                                      _createTextVNode(_toDisplayString(skill.description), 1)
                                    ]),
                                    _: 2
                                  }, 1024))
                                : _createCommentVNode("", true),
                              _createVNode(_component_q_item_label, { caption: "" }, {
                                default: _withCtx(() => [
                                  _createTextVNode(_toDisplayString(skill.name) + " · " + _toDisplayString(skill.status), 1),
                                  (skill.provenance === 'learned')
                                    ? (_openBlock(), _createElementBlock("span", { key: 0 }, " · " + _toDisplayString(_ctx.labels.learned), 1))
                                    : _createCommentVNode("", true),
                                  (skill.shared)
                                    ? (_openBlock(), _createElementBlock("span", { key: 1 }, " · " + _toDisplayString(_ctx.labels.shared), 1))
                                    : _createCommentVNode("", true)
                                ]),
                                _: 2
                              }, 1024)
                            ]),
                            _: 2
                          }, 1024),
                          _createVNode(_component_q_item_section, { side: "" }, {
                            default: _withCtx(() => [
                              _createElementVNode("div", { class: "row no-wrap" }, [
                                (skill.status !== 'retired' && !skill.shared)
                                  ? (_openBlock(), _createBlock(_component_q_btn, {
                                      key: 0,
                                      flat: "",
                                      dense: "",
                                      "no-caps": "",
                                      size: "sm",
                                      label: _ctx.labels.share,
                                      loading: _ctx.busy(skill),
                                      "data-assist-action": "skill-share",
                                      onClick: _withModifiers($event => (_ctx.op('share', skill)), ["stop"])
                                    }, null, 8, ["label", "loading", "onClick"]))
                                  : _createCommentVNode("", true),
                                (skill.status !== 'retired')
                                  ? (_openBlock(), _createBlock(_component_q_btn, {
                                      key: 1,
                                      flat: "",
                                      dense: "",
                                      "no-caps": "",
                                      size: "sm",
                                      color: "negative",
                                      label: _ctx.labels.retire,
                                      disable: _ctx.busy(skill),
                                      "data-assist-action": "skill-retire",
                                      onClick: _withModifiers($event => (_ctx.op('retire', skill)), ["stop"])
                                    }, null, 8, ["label", "disable", "onClick"]))
                                  : _createCommentVNode("", true)
                              ])
                            ]),
                            _: 2
                          }, 1024)
                        ]),
                        _: 2
                      }, 1032, ["data-assist-skill", "data-status", "onClick"]))
                    }), 128))
                  ]),
                  _: 1
                })
              ]))
            }
        } },
        "m-assist-tasks": { hash: "e6fbda68", make: function (Vue) {
            const { toDisplayString: _toDisplayString, openBlock: _openBlock, createElementBlock: _createElementBlock, createCommentVNode: _createCommentVNode, renderList: _renderList, Fragment: _Fragment, createTextVNode: _createTextVNode, resolveComponent: _resolveComponent, withCtx: _withCtx, createVNode: _createVNode, createBlock: _createBlock, createElementVNode: _createElementVNode } = Vue

            return function render(_ctx, _cache) {
              const _component_q_item_label = _resolveComponent("q-item-label")
              const _component_q_item_section = _resolveComponent("q-item-section")
              const _component_q_btn = _resolveComponent("q-btn")
              const _component_q_item = _resolveComponent("q-item")
              const _component_q_list = _resolveComponent("q-list")

              return (_openBlock(), _createElementBlock("div", {
                class: "m-assist-tasks scroll",
                id: "m-assist-tasks-pane"
              }, [
                (!_ctx.groups.length)
                  ? (_openBlock(), _createElementBlock("div", {
                      key: 0,
                      class: "q-pa-md m-muted"
                    }, _toDisplayString(_ctx.labels.nothingWaiting), 1))
                  : _createCommentVNode("", true),
                (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.groups, (group) => {
                  return (_openBlock(), _createElementBlock(_Fragment, {
                    key: group.key
                  }, [
                    _createVNode(_component_q_item_label, {
                      header: "",
                      class: "q-pb-xs"
                    }, {
                      default: _withCtx(() => [
                        _createTextVNode(_toDisplayString(group.label), 1)
                      ]),
                      _: 2
                    }, 1024),
                    _createVNode(_component_q_list, {
                      dense: "",
                      separator: ""
                    }, {
                      default: _withCtx(() => [
                        (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(group.items, (item) => {
                          return (_openBlock(), _createBlock(_component_q_item, {
                            key: (item.approvalId || item.questionId || item.handoffId || item.runId) + group.key,
                            "data-assist-task": item.kind
                          }, {
                            default: _withCtx(() => [
                              _createVNode(_component_q_item_section, null, {
                                default: _withCtx(() => [
                                  _createVNode(_component_q_item_label, { lines: "2" }, {
                                    default: _withCtx(() => [
                                      _createTextVNode(_toDisplayString(item.summary || item.conversationTitle || item.runId), 1)
                                    ]),
                                    _: 2
                                  }, 1024),
                                  _createVNode(_component_q_item_label, {
                                    caption: "",
                                    lines: "1"
                                  }, {
                                    default: _withCtx(() => [
                                      _createTextVNode(_toDisplayString(item.conversationTitle) + _toDisplayString(item.surface ? ' · ' + item.surface : '') + _toDisplayString(item.errorText ? ' · ' + item.errorText : ''), 1)
                                    ]),
                                    _: 2
                                  }, 1024)
                                ]),
                                _: 2
                              }, 1024),
                              _createVNode(_component_q_item_section, {
                                side: "",
                                class: "row no-wrap"
                              }, {
                                default: _withCtx(() => [
                                  _createElementVNode("div", { class: "row no-wrap" }, [
                                    _createVNode(_component_q_btn, {
                                      flat: "",
                                      dense: "",
                                      "no-caps": "",
                                      size: "sm",
                                      color: "primary",
                                      label: _ctx.labels.open,
                                      onClick: $event => (_ctx.openOne(item))
                                    }, null, 8, ["label", "onClick"]),
                                    (group.key === 'running')
                                      ? (_openBlock(), _createBlock(_component_q_btn, {
                                          key: 0,
                                          flat: "",
                                          dense: "",
                                          "no-caps": "",
                                          size: "sm",
                                          label: _ctx.labels.stop,
                                          onClick: $event => (_ctx.cancel(item))
                                        }, null, 8, ["label", "onClick"]))
                                      : _createCommentVNode("", true)
                                  ])
                                ]),
                                _: 2
                              }, 1024)
                            ]),
                            _: 2
                          }, 1032, ["data-assist-task"]))
                        }), 128))
                      ]),
                      _: 2
                    }, 1024)
                  ], 64))
                }), 128))
              ]))
            }
        } },
        "m-assist-thread": { hash: "6ba88647", make: function (Vue) {
            const { resolveComponent: _resolveComponent, createVNode: _createVNode, openBlock: _openBlock, createElementBlock: _createElementBlock, createCommentVNode: _createCommentVNode, toDisplayString: _toDisplayString, renderList: _renderList, Fragment: _Fragment, createTextVNode: _createTextVNode, withCtx: _withCtx, createBlock: _createBlock, createElementVNode: _createElementVNode, withModifiers: _withModifiers, withKeys: _withKeys } = Vue

            return function render(_ctx, _cache) {
              const _component_q_btn = _resolveComponent("q-btn")
              const _component_q_chip = _resolveComponent("q-chip")
              const _component_q_icon = _resolveComponent("q-icon")
              const _component_m_assist_markdown = _resolveComponent("m-assist-markdown")
              const _component_q_tooltip = _resolveComponent("q-tooltip")
              const _component_q_spinner = _resolveComponent("q-spinner")
              const _component_m_assist_card = _resolveComponent("m-assist-card")
              const _component_q_input = _resolveComponent("q-input")

              return (_openBlock(), _createElementBlock("div", {
                class: "m-assist-thread scroll q-pa-sm",
                id: "m-assist-thread",
                role: "log",
                "aria-busy": _ctx.streaming ? 'true' : 'false',
                onScroll: _ctx.onScroll
              }, [
                (_ctx.convo.hasEarlier)
                  ? (_openBlock(), _createElementBlock("div", {
                      key: 0,
                      class: "text-center q-mb-sm"
                    }, [
                      _createVNode(_component_q_btn, {
                        flat: "",
                        dense: "",
                        "no-caps": "",
                        size: "sm",
                        label: _ctx.labels.earlier,
                        onClick: _ctx.earlier
                      }, null, 8, ["label", "onClick"])
                    ]))
                  : _createCommentVNode("", true),
                (!_ctx.rows.length && !_ctx.loose.length)
                  ? (_openBlock(), _createElementBlock("div", {
                      key: 1,
                      class: "m-assist-empty text-center q-pa-lg m-muted",
                      id: "m-assist-empty"
                    }, [
                      _createTextVNode(_toDisplayString(_ctx.labels.empty), 1),
                      (_ctx.prompts.length)
                        ? (_openBlock(), _createElementBlock("div", {
                            key: 0,
                            class: "q-mt-md m-assist-prompts",
                            id: "m-assist-prompts-empty"
                          }, [
                            (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.prompts, (prompt, index) => {
                              return (_openBlock(), _createBlock(_component_q_chip, {
                                key: prompt.promptId || index,
                                clickable: "",
                                dense: "",
                                icon: prompt.send ? 'sym_o_send' : undefined,
                                "data-assist-prompt": "",
                                "data-send": prompt.send ? 'true' : 'false',
                                onClick: $event => (_ctx.choosePrompt(prompt))
                              }, {
                                default: _withCtx(() => [
                                  _createTextVNode(_toDisplayString(prompt.label || prompt.text), 1)
                                ]),
                                _: 2
                              }, 1032, ["icon", "data-send", "onClick"]))
                            }), 128))
                          ]))
                        : _createCommentVNode("", true)
                    ]))
                  : _createCommentVNode("", true),
                (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.rows, (row) => {
                  return (_openBlock(), _createElementBlock(_Fragment, {
                    key: row.key
                  }, [
                    (row.kind === 'user')
                      ? (_openBlock(), _createElementBlock("div", {
                          key: 0,
                          class: "m-assist-user q-mb-sm",
                          "data-assist-item": "user",
                          "data-assist-local": row.local || undefined
                        }, [
                          _createElementVNode("div", { class: "m-assist-bubble" }, [
                            (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(row.segments, (segment, index) => {
                              return (_openBlock(), _createElementBlock(_Fragment, { key: index }, [
                                (segment.text !== undefined)
                                  ? (_openBlock(), _createElementBlock("span", {
                                      key: 0,
                                      class: "m-assist-pre"
                                    }, _toDisplayString(segment.text), 1))
                                  : (_openBlock(), _createBlock(_component_q_chip, {
                                      key: 1,
                                      dense: "",
                                      square: "",
                                      size: "sm",
                                      icon: "sym_o_link",
                                      class: "m-assist-record-chip"
                                    }, {
                                      default: _withCtx(() => [
                                        _createTextVNode(_toDisplayString(segment.chip.label), 1)
                                      ]),
                                      _: 2
                                    }, 1024))
                              ], 64))
                            }), 128))
                          ]),
                          (row.files && row.files.length)
                            ? (_openBlock(), _createElementBlock("div", {
                                key: 0,
                                class: "row justify-end q-gutter-xs q-mt-xs m-assist-files"
                              }, [
                                (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(row.files, (file) => {
                                  return (_openBlock(), _createElementBlock("a", {
                                    key: file.attachmentId,
                                    class: "m-assist-file",
                                    href: _ctx.attachmentUrl(file.attachmentId),
                                    download: file.fileName || true,
                                    target: "_blank",
                                    rel: "noopener",
                                    "data-assist-file": file.attachmentId,
                                    title: file.mediaType
                                  }, [
                                    _createVNode(_component_q_icon, {
                                      name: "sym_o_attach_file",
                                      size: "xs"
                                    }),
                                    _createTextVNode(_toDisplayString(file.fileName) + " ", 1),
                                    _createElementVNode("span", { class: "m-muted" }, _toDisplayString(_ctx.sizeText(file.size)), 1)
                                  ], 8, ["href", "download", "data-assist-file", "title"]))
                                }), 128))
                              ]))
                            : _createCommentVNode("", true),
                          (row.local === 'sending')
                            ? (_openBlock(), _createElementBlock("div", {
                                key: 1,
                                class: "text-caption m-muted text-right"
                              }, _toDisplayString(_ctx.labels.sending), 1))
                            : (row.local === 'failed')
                              ? (_openBlock(), _createElementBlock("div", {
                                  key: 2,
                                  class: "text-caption text-negative text-right"
                                }, [
                                  _createTextVNode(_toDisplayString(_ctx.labels.notSent) + _toDisplayString(row.error ? ': ' + row.error : ''), 1),
                                  _createVNode(_component_q_btn, {
                                    flat: "",
                                    dense: "",
                                    "no-caps": "",
                                    size: "sm",
                                    label: _ctx.labels.retry,
                                    "data-assist-action": "resend",
                                    onClick: $event => (_ctx.resend(row))
                                  }, null, 8, ["label", "onClick"]),
                                  _createVNode(_component_q_btn, {
                                    flat: "",
                                    dense: "",
                                    round: "",
                                    size: "sm",
                                    icon: "sym_o_close",
                                    onClick: $event => (_ctx.drop(row))
                                  }, null, 8, ["onClick"])
                                ]))
                              : _createCommentVNode("", true)
                        ], 8, ["data-assist-local"]))
                      : (row.kind === 'text')
                        ? (_openBlock(), _createElementBlock("div", {
                            key: 1,
                            class: "m-assist-answer q-mb-sm",
                            "data-assist-item": "assistant",
                            "data-message-id": row.messageId
                          }, [
                            _createVNode(_component_m_assist_markdown, {
                              text: row.text,
                              invented: row.invented,
                              chips: row.chips,
                              streaming: row.streaming
                            }, null, 8, ["text", "invented", "chips", "streaming"]),
                            (!row.streaming && row.messageId && row.text)
                              ? (_openBlock(), _createElementBlock("div", {
                                  key: 0,
                                  class: "row items-center no-wrap m-assist-actions"
                                }, [
                                  _createVNode(_component_q_btn, {
                                    flat: "",
                                    round: "",
                                    dense: "",
                                    size: "xs",
                                    icon: "sym_o_content_copy",
                                    "aria-label": _ctx.labels.copy,
                                    onClick: $event => (_ctx.copy(row.text))
                                  }, null, 8, ["aria-label", "onClick"]),
                                  _createVNode(_component_q_btn, {
                                    flat: "",
                                    round: "",
                                    dense: "",
                                    size: "xs",
                                    icon: "sym_o_thumb_up",
                                    color: row.feedback && row.feedback.rating === 'up' ? 'primary' : undefined,
                                    "aria-label": _ctx.labels.up,
                                    onClick: $event => (_ctx.rate(row, 'up'))
                                  }, null, 8, ["color", "aria-label", "onClick"]),
                                  _createVNode(_component_q_btn, {
                                    flat: "",
                                    round: "",
                                    dense: "",
                                    size: "xs",
                                    icon: "sym_o_thumb_down",
                                    color: row.feedback && row.feedback.rating === 'down' ? 'negative' : undefined,
                                    "aria-label": _ctx.labels.down,
                                    onClick: $event => (_ctx.rate(row, 'down'))
                                  }, null, 8, ["color", "aria-label", "onClick"]),
                                  (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.responseActions, (action) => {
                                    return (_openBlock(), _createBlock(_component_q_btn, {
                                      key: action.key,
                                      flat: "",
                                      round: "",
                                      dense: "",
                                      size: "xs",
                                      icon: action.icon || 'sym_o_bolt',
                                      "aria-label": action.label,
                                      "data-assist-response-action": action.key,
                                      onClick: $event => (_ctx.respond(action, row))
                                    }, {
                                      default: _withCtx(() => [
                                        _createVNode(_component_q_tooltip, null, {
                                          default: _withCtx(() => [
                                            _createTextVNode(_toDisplayString(action.label), 1)
                                          ]),
                                          _: 2
                                        }, 1024)
                                      ]),
                                      _: 2
                                    }, 1032, ["icon", "aria-label", "data-assist-response-action", "onClick"]))
                                  }), 128))
                                ]))
                              : _createCommentVNode("", true)
                          ], 8, ["data-message-id"]))
                        : (row.kind === 'reasoning' || row.kind === 'thinking')
                          ? (_openBlock(), _createElementBlock("div", {
                              key: 2,
                              class: "m-assist-thought text-caption m-muted q-mb-xs",
                              "data-assist-item": "reasoning"
                            }, [
                              _createElementVNode("span", {
                                class: "cursor-pointer",
                                onClick: $event => (_ctx.toggleStep(row.key))
                              }, [
                                _createVNode(_component_q_icon, {
                                  name: "sym_o_psychology",
                                  size: "xs"
                                }),
                                _createTextVNode(" " + _toDisplayString(_ctx.labels.thought), 1)
                              ], 8, ["onClick"]),
                              (_ctx.openSteps[row.key] || row.kind === 'thinking')
                                ? (_openBlock(), _createElementBlock("div", {
                                    key: 0,
                                    class: "m-assist-pre q-pl-md"
                                  }, _toDisplayString(row.text), 1))
                                : _createCommentVNode("", true)
                            ]))
                          : (row.kind === 'step')
                            ? (_openBlock(), _createElementBlock(_Fragment, { key: 3 }, [
                                _createElementVNode("div", {
                                  class: "m-assist-step text-caption q-mb-xs",
                                  "data-assist-item": "step",
                                  "data-tool": row.name,
                                  "data-status": row.status
                                }, [
                                  _createElementVNode("div", {
                                    class: "row items-center no-wrap cursor-pointer",
                                    onClick: $event => (_ctx.toggleStep(row.key))
                                  }, [
                                    (row.status === 'running')
                                      ? (_openBlock(), _createBlock(_component_q_spinner, {
                                          key: 0,
                                          size: "14px",
                                          class: "q-mr-xs"
                                        }))
                                      : (_openBlock(), _createBlock(_component_q_icon, {
                                          key: 1,
                                          name: _ctx.stepIcon(row.status),
                                          size: "xs",
                                          color: row.isError ? 'negative' : undefined,
                                          class: "q-mr-xs"
                                        }, null, 8, ["name", "color"])),
                                    _createElementVNode("span", { class: "ellipsis" }, _toDisplayString(row.title), 1)
                                  ], 8, ["onClick"]),
                                  (_ctx.openSteps[row.key])
                                    ? (_openBlock(), _createElementBlock("div", {
                                        key: 0,
                                        class: "m-assist-step-detail q-pl-md"
                                      }, [
                                        _createElementVNode("div", { class: "m-assist-pre" }, _toDisplayString(row.arguments), 1),
                                        (row.result)
                                          ? (_openBlock(), _createElementBlock("div", {
                                              key: 0,
                                              class: "m-assist-pre q-mt-xs"
                                            }, _toDisplayString(row.result), 1))
                                          : _createCommentVNode("", true),
                                        _createVNode(_component_q_btn, {
                                          flat: "",
                                          dense: "",
                                          "no-caps": "",
                                          size: "xs",
                                          icon: "sym_o_content_copy",
                                          label: _ctx.labels.copy,
                                          onClick: $event => (_ctx.copy(row.arguments + '\n' + row.result))
                                        }, null, 8, ["label", "onClick"])
                                      ]))
                                    : _createCommentVNode("", true)
                                ], 8, ["data-tool", "data-status"]),
                                (_ctx.approvalsByCall[row.toolCallId])
                                  ? (_openBlock(), _createBlock(_component_m_assist_card, {
                                      key: 0,
                                      kind: "approval",
                                      card: _ctx.approvalsByCall[row.toolCallId]
                                    }, null, 8, ["card"]))
                                  : _createCommentVNode("", true)
                              ], 64))
                            : (row.kind === 'done')
                              ? (_openBlock(), _createElementBlock("div", {
                                  key: 4,
                                  class: "m-assist-done text-caption m-muted q-mb-sm",
                                  "data-assist-item": "done",
                                  "data-state": row.state
                                }, [
                                  (row.failed)
                                    ? (_openBlock(), _createElementBlock("span", {
                                        key: 0,
                                        class: "text-negative"
                                      }, _toDisplayString(row.errorText || row.errorCode || _ctx.labels.errorTitle), 1))
                                    : _createCommentVNode("", true),
                                  _createTextVNode(_toDisplayString(_ctx.doneText(row)), 1),
                                  (row.failed)
                                    ? (_openBlock(), _createBlock(_component_q_btn, {
                                        key: 1,
                                        flat: "",
                                        dense: "",
                                        "no-caps": "",
                                        size: "sm",
                                        label: _ctx.labels.retry,
                                        "data-assist-action": "retry",
                                        onClick: _ctx.retry
                                      }, null, 8, ["label", "onClick"]))
                                    : _createCommentVNode("", true),
                                  (row.completed && _ctx.saveSkillOn)
                                    ? (_openBlock(), _createBlock(_component_q_btn, {
                                        key: 2,
                                        flat: "",
                                        dense: "",
                                        "no-caps": "",
                                        size: "sm",
                                        icon: "sym_o_school",
                                        label: _ctx.labels.saveSkill,
                                        "data-assist-action": "save-skill",
                                        onClick: $event => (_ctx.saveSkill(row))
                                      }, null, 8, ["label", "onClick"]))
                                    : _createCommentVNode("", true)
                                ], 8, ["data-state"]))
                              : (row.kind === 'queued')
                                ? (_openBlock(), _createElementBlock("div", {
                                    key: 5,
                                    class: "m-assist-user m-assist-queued q-mb-sm",
                                    "data-assist-item": "queued",
                                    "data-input-id": row.inputId
                                  }, [
                                    (_ctx.editing === row.key)
                                      ? (_openBlock(), _createElementBlock("div", {
                                          key: 0,
                                          class: "m-assist-queued-edit"
                                        }, [
                                          _createVNode(_component_q_input, {
                                            modelValue: _ctx.editText,
                                            "onUpdate:modelValue": $event => ((_ctx.editText) = $event),
                                            type: "textarea",
                                            autogrow: "",
                                            dense: "",
                                            outlined: "",
                                            autofocus: "",
                                            "data-assist-queued-text": "",
                                            onKeydown: [
                                              _withKeys(_withModifiers($event => (_ctx.saveQueued(row)), ["exact","prevent"]), ["enter"]),
                                              _withKeys(_withModifiers($event => (_ctx.editing = null), ["prevent","stop"]), ["esc"])
                                            ]
                                          }, null, 8, ["modelValue", "onUpdate:modelValue", "onKeydown"]),
                                          _createElementVNode("div", { class: "row justify-end q-gutter-xs q-mt-xs" }, [
                                            _createVNode(_component_q_btn, {
                                              flat: "",
                                              dense: "",
                                              "no-caps": "",
                                              size: "sm",
                                              label: _ctx.labels.cancel,
                                              onClick: $event => (_ctx.editing = null)
                                            }, null, 8, ["label", "onClick"]),
                                            _createVNode(_component_q_btn, {
                                              unelevated: "",
                                              dense: "",
                                              "no-caps": "",
                                              size: "sm",
                                              color: "primary",
                                              label: _ctx.labels.save,
                                              loading: _ctx.busyQueued(row),
                                              disable: !_ctx.editText.trim(),
                                              "data-assist-action": "queued-save",
                                              onClick: $event => (_ctx.saveQueued(row))
                                            }, null, 8, ["label", "loading", "disable", "onClick"])
                                          ])
                                        ]))
                                      : (_openBlock(), _createElementBlock("div", {
                                          key: 1,
                                          class: "m-assist-bubble"
                                        }, [
                                          _createElementVNode("span", { class: "m-assist-pre" }, _toDisplayString(row.text), 1)
                                        ])),
                                    _createElementVNode("div", { class: "text-caption m-muted text-right" }, [
                                      _createTextVNode(_toDisplayString(_ctx.labels.queued), 1),
                                      (row.inputId && _ctx.queuedOn && _ctx.editing !== row.key)
                                        ? (_openBlock(), _createElementBlock(_Fragment, { key: 0 }, [
                                            _createVNode(_component_q_btn, {
                                              flat: "",
                                              dense: "",
                                              "no-caps": "",
                                              size: "sm",
                                              label: _ctx.labels.edit,
                                              "data-assist-action": "queued-edit",
                                              onClick: $event => (_ctx.editQueued(row))
                                            }, null, 8, ["label", "onClick"]),
                                            _createVNode(_component_q_btn, {
                                              flat: "",
                                              dense: "",
                                              "no-caps": "",
                                              size: "sm",
                                              label: _ctx.labels.cancel,
                                              loading: _ctx.busyQueued(row),
                                              "data-assist-action": "queued-cancel",
                                              onClick: $event => (_ctx.cancelQueued(row))
                                            }, null, 8, ["label", "loading", "onClick"])
                                          ], 64))
                                        : _createCommentVNode("", true)
                                    ])
                                  ], 8, ["data-input-id"]))
                                : (row.kind === 'note' && row.memoryId)
                                  ? (_openBlock(), _createElementBlock("div", {
                                      key: 6,
                                      class: "m-assist-note text-caption q-mb-sm row items-center no-wrap",
                                      "data-assist-item": "note",
                                      "data-assist-remembered": row.memoryId
                                    }, [
                                      _createElementVNode("span", { class: "col" }, _toDisplayString(row.forgotten ? _ctx.labels.forgotten : _ctx.labels.remembered) + ": " + _toDisplayString(row.remembered), 1),
                                      (!row.forgotten)
                                        ? (_openBlock(), _createBlock(_component_q_btn, {
                                            key: 0,
                                            flat: "",
                                            dense: "",
                                            "no-caps": "",
                                            size: "sm",
                                            color: "primary",
                                            label: _ctx.labels.forget,
                                            "data-assist-action": "remembered-forget",
                                            onClick: $event => (_ctx.forgetRemembered(row))
                                          }, null, 8, ["label", "onClick"]))
                                        : _createCommentVNode("", true)
                                    ], 8, ["data-assist-remembered"]))
                                  : (row.kind === 'note' && row.lessonId)
                                    ? (_openBlock(), _createElementBlock("div", {
                                        key: 7,
                                        class: "m-assist-note text-caption q-mb-sm",
                                        "data-assist-item": "note",
                                        "data-assist-lesson": row.lessonId
                                      }, _toDisplayString(_ctx.labels.lessonKept) + " " + _toDisplayString(row.anchor) + ": " + _toDisplayString(row.lesson), 9, ["data-assist-lesson"]))
                                    : (row.kind === 'note')
                                      ? (_openBlock(), _createElementBlock("div", {
                                          key: 8,
                                          class: "m-assist-note text-caption q-mb-sm m-assist-pre",
                                          "data-assist-item": "note"
                                        }, _toDisplayString(row.text), 1))
                                      : _createCommentVNode("", true)
                  ], 64))
                }), 128)),
                (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.loose, (card) => {
                  return (_openBlock(), _createBlock(_component_m_assist_card, {
                    key: 'a' + card.approvalId,
                    kind: "approval",
                    card: card
                  }, null, 8, ["card"]))
                }), 128))
              ], 40, ["aria-busy", "onScroll"]))
            }
        } },
        "m-avatar": { hash: "71857812", make: function (Vue) {
            const { openBlock: _openBlock, createElementBlock: _createElementBlock, createCommentVNode: _createCommentVNode, toDisplayString: _toDisplayString, resolveComponent: _resolveComponent, createBlock: _createBlock, normalizeClass: _normalizeClass, createTextVNode: _createTextVNode, withCtx: _withCtx } = Vue

            return function render(_ctx, _cache) {
              const _component_q_icon = _resolveComponent("q-icon")
              const _component_q_tooltip = _resolveComponent("q-tooltip")
              const _component_q_avatar = _resolveComponent("q-avatar")

              return (_openBlock(), _createBlock(_component_q_avatar, {
                class: "m-avatar",
                size: _ctx.size,
                color: _ctx.background,
                "text-color": _ctx.showImage ? undefined : _ctx.textColor,
                square: _ctx.square,
                rounded: _ctx.rounded,
                "aria-label": _ctx.name || undefined
              }, {
                default: _withCtx(() => [
                  (_ctx.showImage)
                    ? (_openBlock(), _createElementBlock("img", {
                        key: 0,
                        src: _ctx.resolvedImage,
                        alt: _ctx.name || '',
                        onError: $event => (_ctx.failedImage = _ctx.resolvedImage)
                      }, null, 40, ["src", "alt", "onError"]))
                    : (_ctx.initials)
                      ? (_openBlock(), _createElementBlock("span", {
                          key: 1,
                          class: "m-avatar-initials"
                        }, _toDisplayString(_ctx.initials), 1))
                      : (_openBlock(), _createBlock(_component_q_icon, {
                          key: 2,
                          name: "sym_o_person"
                        })),
                  (_ctx.shownPresence)
                    ? (_openBlock(), _createElementBlock("span", {
                        key: 3,
                        class: _normalizeClass(["m-avatar-presence", 'm-presence-' + _ctx.shownPresence]),
                        title: _ctx.shownPresence
                      }, null, 10, ["title"]))
                    : _createCommentVNode("", true),
                  (_ctx.tooltip && _ctx.name)
                    ? (_openBlock(), _createBlock(_component_q_tooltip, { key: 4 }, {
                        default: _withCtx(() => [
                          _createTextVNode(_toDisplayString(_ctx.name), 1)
                        ]),
                        _: 1
                      }))
                    : _createCommentVNode("", true)
                ]),
                _: 1
              }, 8, ["size", "color", "text-color", "square", "rounded", "aria-label"]))
            }
        } },
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
        "m-chatter": { hash: "8482661e", make: function (Vue) {
            const { resolveComponent: _resolveComponent, createVNode: _createVNode, normalizeClass: _normalizeClass, openBlock: _openBlock, createElementBlock: _createElementBlock, createCommentVNode: _createCommentVNode } = Vue

            return function render(_ctx, _cache) {
              const _component_m_dynamic_container = _resolveComponent("m-dynamic-container")

              return (_ctx.loadUrl)
                ? (_openBlock(), _createElementBlock("div", {
                    key: 0,
                    class: _normalizeClass('m-record-panel m-record-panel-' + _ctx.kind)
                  }, [
                    _createVNode(_component_m_dynamic_container, {
                      ref: "container",
                      id: _ctx.containerId,
                      url: _ctx.loadUrl
                    }, null, 8, ["id", "url"])
                  ], 2))
                : _createCommentVNode("", true)
            }
        } },
        "m-check": { hash: "2b541cd8", make: function (Vue) {
            const { resolveComponent: _resolveComponent, openBlock: _openBlock, createBlock: _createBlock, createCommentVNode: _createCommentVNode, toDisplayString: _toDisplayString, createTextVNode: _createTextVNode, withCtx: _withCtx, createVNode: _createVNode, mergeProps: _mergeProps } = Vue

            return function render(_ctx, _cache) {
              const _component_q_toggle = _resolveComponent("q-toggle")
              const _component_q_checkbox = _resolveComponent("q-checkbox")
              const _component_q_tooltip = _resolveComponent("q-tooltip")
              const _component_q_option_group = _resolveComponent("q-option-group")
              const _component_m_field = _resolveComponent("m-field")

              return (!_ctx.options.length && _ctx.toggle)
                ? (_openBlock(), _createBlock(_component_q_toggle, {
                    key: 0,
                    dense: "",
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
                : (!_ctx.options.length)
                  ? (_openBlock(), _createBlock(_component_q_checkbox, {
                      key: 1,
                      dense: "",
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
                        key: 2,
                        borderless: "",
                        class: "m-field-group",
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
                          _createVNode(_component_q_option_group, {
                            type: "checkbox",
                            inline: "",
                            dense: "",
                            size: "sm",
                            options: _ctx.options,
                            "model-value": _ctx.local,
                            "onUpdate:modelValue": _ctx.onInput,
                            name: _ctx.name,
                            id: _ctx.id,
                            disable: _ctx.disable
                          }, null, 8, ["options", "model-value", "onUpdate:modelValue", "name", "id", "disable"])
                        ]),
                        _: 1
                      }, 16, ["label", "bg-color", "disable"]))
                    : (_openBlock(), _createBlock(_component_q_option_group, {
                        key: 3,
                        type: "checkbox",
                        inline: "",
                        dense: "",
                        size: "sm",
                        options: _ctx.options,
                        "model-value": _ctx.local,
                        "onUpdate:modelValue": _ctx.onInput,
                        name: _ctx.name,
                        id: _ctx.id,
                        disable: _ctx.disable
                      }, null, 8, ["options", "model-value", "onUpdate:modelValue", "name", "id", "disable"]))
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
        "m-code": { hash: "83b1b6c5", make: function (Vue) {
            const { renderList: _renderList, Fragment: _Fragment, openBlock: _openBlock, createElementBlock: _createElementBlock, createCommentVNode: _createCommentVNode, createElementVNode: _createElementVNode, normalizeClass: _normalizeClass } = Vue

            return function render(_ctx, _cache) {
              return (_openBlock(), _createElementBlock("div", {
                class: _normalizeClass(["m-code", {'m-code-numbered': _ctx.lineNumbers}]),
                id: _ctx.id
              }, [
                _createElementVNode("table", { class: "m-code-table" }, [
                  _createElementVNode("tbody", null, [
                    (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.lines, (line, index) => {
                      return (_openBlock(), _createElementBlock("tr", {
                        key: index,
                        class: _normalizeClass({'m-code-marked': _ctx.marked.has(index + 1)}),
                        "data-line-row": index + 1
                      }, [
                        (_ctx.lineNumbers)
                          ? (_openBlock(), _createElementBlock("td", {
                              key: 0,
                              class: "m-code-number",
                              "data-line": index + 1
                            }, null, 8, ["data-line"]))
                          : _createCommentVNode("", true),
                        _createElementVNode("td", { class: "m-code-line" }, [
                          _createElementVNode("code", {
                            class: "hljs",
                            innerHTML: line || ' '
                          }, null, 8, ["innerHTML"])
                        ])
                      ], 10, ["data-line-row"]))
                    }), 128))
                  ])
                ])
              ], 10, ["id"]))
            }
        } },
        "m-coedit": { hash: "148d829f", make: function (Vue) {
            const { renderList: _renderList, Fragment: _Fragment, openBlock: _openBlock, createElementBlock: _createElementBlock, resolveComponent: _resolveComponent, createBlock: _createBlock, createVNode: _createVNode, toDisplayString: _toDisplayString, createElementVNode: _createElementVNode, mergeProps: _mergeProps, createCommentVNode: _createCommentVNode } = Vue

            return function render(_ctx, _cache) {
              const _component_m_avatar = _resolveComponent("m-avatar")
              const _component_q_icon = _resolveComponent("q-icon")
              const _component_m_rich_text = _resolveComponent("m-rich-text")

              return (_openBlock(), _createElementBlock("div", {
                class: "m-coedit",
                "data-state": _ctx.status.state
              }, [
                _createElementVNode("div", {
                  class: "m-coedit-presence row items-center no-wrap",
                  role: "status"
                }, [
                  (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.people, (person) => {
                    return (_openBlock(), _createBlock(_component_m_avatar, {
                      key: person.key,
                      class: "m-coedit-peer",
                      name: person.name,
                      "user-id": person.userId,
                      size: "22px",
                      tooltip: ""
                    }, null, 8, ["name", "user-id"]))
                  }), 128)),
                  _createVNode(_component_q_icon, {
                    name: _ctx.stateIcon,
                    size: "16px",
                    class: "m-coedit-state-icon"
                  }, null, 8, ["name"]),
                  _createElementVNode("span", { class: "m-coedit-state text-caption" }, _toDisplayString(_ctx.t(_ctx.stateLabel)), 1)
                ]),
                (_ctx.collab)
                  ? (_openBlock(), _createBlock(_component_m_rich_text, _mergeProps({
                      key: 0,
                      ref: "editor"
                    }, _ctx.$attrs, {
                      collab: _ctx.collab,
                      light: _ctx.light,
                      placeholder: _ctx.placeholder,
                      rows: _ctx.rows,
                      readonly: _ctx.readonly || (_ctx.joinedOnce && !_ctx.canEdit),
                      disable: !_ctx.joinedOnce,
                      "plain-toggle": false,
                      onFailed: _ctx.editorFailed
                    }), null, 16, ["collab", "light", "placeholder", "rows", "readonly", "disable", "onFailed"]))
                  : _createCommentVNode("", true)
              ], 8, ["data-state"]))
            }
        } },
        "m-container-box": { hash: "faa2a157", make: function (Vue) {
            const { toDisplayString: _toDisplayString, openBlock: _openBlock, createElementBlock: _createElementBlock, createCommentVNode: _createCommentVNode, renderSlot: _renderSlot, resolveComponent: _resolveComponent, createVNode: _createVNode, withModifiers: _withModifiers, createElementVNode: _createElementVNode, withCtx: _withCtx, vShow: _vShow, withDirectives: _withDirectives, normalizeClass: _normalizeClass, createBlock: _createBlock } = Vue

            return function render(_ctx, _cache) {
              const _component_q_space = _resolveComponent("q-space")
              const _component_q_btn = _resolveComponent("q-btn")
              const _component_q_separator = _resolveComponent("q-separator")
              const _component_q_card_section = _resolveComponent("q-card-section")
              const _component_q_slide_transition = _resolveComponent("q-slide-transition")
              const _component_q_card = _resolveComponent("q-card")

              return (_openBlock(), _createBlock(_component_q_card, {
                flat: "",
                bordered: "",
                class: _normalizeClass(["q-ma-sm m-container-box", _ctx.boxClass])
              }, {
                default: _withCtx(() => [
                  _createElementVNode("div", {
                    class: "m-box-head row items-center no-wrap q-py-xs q-px-sm",
                    onClick: _withModifiers(_ctx.toggleBody, ["self"])
                  }, [
                    (_ctx.title && _ctx.title.length)
                      ? (_openBlock(), _createElementBlock("div", {
                          key: 0,
                          class: "text-subtitle1 text-weight-medium ellipsis cursor-pointer",
                          onClick: _ctx.toggleBody
                        }, _toDisplayString(_ctx.title), 9, ["onClick"]))
                      : _createCommentVNode("", true),
                    _renderSlot(_ctx.$slots, "header"),
                    _createVNode(_component_q_space),
                    _renderSlot(_ctx.$slots, "toolbar"),
                    _createVNode(_component_q_btn, {
                      flat: "",
                      round: "",
                      dense: "",
                      size: "sm",
                      color: "grey-7",
                      icon: _ctx.isBodyOpen ? 'expand_less' : 'expand_more',
                      onClick: _ctx.toggleBody
                    }, null, 8, ["icon", "onClick"])
                  ], 8, ["onClick"]),
                  _createVNode(_component_q_slide_transition, null, {
                    default: _withCtx(() => [
                      _withDirectives(_createElementVNode("div", null, [
                        _createVNode(_component_q_separator),
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
                  })
                ]),
                _: 3
              }, 8, ["class"]))
            }
        } },
        "m-container-dialog": { hash: "416fea64", make: function (Vue) {
            const { renderSlot: _renderSlot, resolveComponent: _resolveComponent, normalizeClass: _normalizeClass, createVNode: _createVNode, toDisplayString: _toDisplayString, createTextVNode: _createTextVNode, withCtx: _withCtx, openBlock: _openBlock, createBlock: _createBlock, createCommentVNode: _createCommentVNode, createElementVNode: _createElementVNode, createElementBlock: _createElementBlock } = Vue

            return function render(_ctx, _cache) {
              const _component_q_btn = _resolveComponent("q-btn")
              const _component_q_tooltip = _resolveComponent("q-tooltip")
              const _component_m_dialog = _resolveComponent("m-dialog")

              return (_openBlock(), _createElementBlock("span", null, [
                _createElementVNode("span", {
                  onClick: $event => (_ctx.show())
                }, [
                  _renderSlot(_ctx.$slots, "button", {}, () => [
                    _createVNode(_component_q_btn, {
                      dense: "",
                      unelevated: "",
                      "no-caps": "",
                      icon: _ctx.buttonIcon,
                      label: _ctx.buttonText,
                      class: _normalizeClass(_ctx.moqui.tonalClass(_ctx.color, _ctx.buttonClass))
                    }, null, 8, ["icon", "label", "class"])
                  ]),
                  (_ctx.tooltip)
                    ? (_openBlock(), _createBlock(_component_q_tooltip, { key: 0 }, {
                        default: _withCtx(() => [
                          _createTextVNode(_toDisplayString(_ctx.tooltip), 1)
                        ]),
                        _: 1
                      }))
                    : _createCommentVNode("", true)
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
        "m-container-split": { hash: "7ec2b534", make: function (Vue) {
            const { resolveComponent: _resolveComponent, openBlock: _openBlock, createBlock: _createBlock, createCommentVNode: _createCommentVNode, renderSlot: _renderSlot, vShow: _vShow, withDirectives: _withDirectives, createElementBlock: _createElementBlock, normalizeStyle: _normalizeStyle, withCtx: _withCtx, createVNode: _createVNode, normalizeClass: _normalizeClass } = Vue

            return function render(_ctx, _cache) {
              const _component_q_btn = _resolveComponent("q-btn")
              const _component_q_splitter = _resolveComponent("q-splitter")

              return (_openBlock(), _createElementBlock("div", {
                ref: "root",
                class: _normalizeClass(["m-container-split", {'m-split-stacked': _ctx.layout.stacked, 'm-split-right': _ctx.right, 'm-split-selected': _ctx.selected}]),
                id: _ctx.id,
                "data-stacked": _ctx.layout.stacked ? 'true' : 'false'
              }, [
                _createVNode(_component_q_splitter, {
                  "model-value": _ctx.layout.side,
                  "onUpdate:modelValue": _ctx.drag,
                  unit: "px",
                  limits: [_ctx.layout.min, _ctx.layout.max],
                  reverse: _ctx.right,
                  disable: _ctx.layout.stacked,
                  style: _normalizeStyle(_ctx.paneHeight ? {height: _ctx.paneHeight} : undefined),
                  "separator-class": "m-split-separator"
                }, {
                  before: _withCtx(() => [
                    (_ctx.right)
                      ? _withDirectives((_openBlock(), _createElementBlock("div", {
                          key: 0,
                          class: "m-split-main"
                        }, [
                          (_ctx.backShown)
                            ? (_openBlock(), _createBlock(_component_q_btn, {
                                key: 0,
                                flat: "",
                                dense: "",
                                "no-caps": "",
                                icon: "arrow_back",
                                label: _ctx.backLabel,
                                class: "m-split-back",
                                onClick: _ctx.back
                              }, null, 8, ["label", "onClick"]))
                            : _createCommentVNode("", true),
                          _renderSlot(_ctx.$slots, "main")
                        ], 512)), [
                          [_vShow, _ctx.showMain]
                        ])
                      : _withDirectives((_openBlock(), _createElementBlock("div", {
                          key: 1,
                          class: "m-split-side"
                        }, [
                          _renderSlot(_ctx.$slots, "side")
                        ], 512)), [
                          [_vShow, _ctx.showSide]
                        ])
                  ]),
                  after: _withCtx(() => [
                    (_ctx.right)
                      ? _withDirectives((_openBlock(), _createElementBlock("div", {
                          key: 0,
                          class: "m-split-side"
                        }, [
                          _renderSlot(_ctx.$slots, "side")
                        ], 512)), [
                          [_vShow, _ctx.showSide]
                        ])
                      : _withDirectives((_openBlock(), _createElementBlock("div", {
                          key: 1,
                          class: "m-split-main"
                        }, [
                          (_ctx.backShown)
                            ? (_openBlock(), _createBlock(_component_q_btn, {
                                key: 0,
                                flat: "",
                                dense: "",
                                "no-caps": "",
                                icon: "arrow_back",
                                label: _ctx.backLabel,
                                class: "m-split-back",
                                onClick: _ctx.back
                              }, null, 8, ["label", "onClick"]))
                            : _createCommentVNode("", true),
                          _renderSlot(_ctx.$slots, "main")
                        ], 512)), [
                          [_vShow, _ctx.showMain]
                        ])
                  ]),
                  _: 3
                }, 8, ["model-value", "onUpdate:modelValue", "limits", "reverse", "disable", "style"])
              ], 10, ["id", "data-stacked"]))
            }
        } },
        "m-date-period": { hash: "9a3df286", make: function (Vue) {
            const { resolveComponent: _resolveComponent, createVNode: _createVNode, createTextVNode: _createTextVNode, withCtx: _withCtx, openBlock: _openBlock, createElementBlock: _createElementBlock, createCommentVNode: _createCommentVNode, toDisplayString: _toDisplayString, createBlock: _createBlock, createElementVNode: _createElementVNode } = Vue

            return function render(_ctx, _cache) {
              const _component_m_date_time = _resolveComponent("m-date-time")
              const _component_q_icon = _resolveComponent("q-icon")
              const _component_q_tooltip = _resolveComponent("q-tooltip")
              const _component_q_btn = _resolveComponent("q-btn")
              const _component_q_date = _resolveComponent("q-date")
              const _component_q_popup_proxy = _resolveComponent("q-popup-proxy")
              const _component_q_select = _resolveComponent("q-select")
              const _component_q_menu = _resolveComponent("q-menu")
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
                          flat: "",
                          round: "",
                          dense: "",
                          size: "sm",
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
                          flat: "",
                          round: "",
                          dense: "",
                          size: "sm",
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
                      "hide-bottom-space": "",
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
                      prepend: _withCtx(() => [
                        _createVNode(_component_q_icon, {
                          name: "event",
                          size: "xs",
                          class: "cursor-pointer"
                        }, {
                          default: _withCtx(() => [
                            _createVNode(_component_q_popup_proxy, {
                              ref: "qDateProxy",
                              cover: "",
                              "transition-show": "scale",
                              "transition-hide": "scale"
                            }, {
                              default: _withCtx(() => [
                                _createVNode(_component_q_date, {
                                  modelValue: _ctx.fields[_ctx.name+'_pdate'],
                                  "onUpdate:modelValue": [$event => ((_ctx.fields[_ctx.name+'_pdate']) = $event), _ctx.hideDate],
                                  mask: "YYYY-MM-DD",
                                  "today-btn": ""
                                }, null, 8, ["modelValue", "onUpdate:modelValue"])
                              ]),
                              _: 1
                            }, 512)
                          ]),
                          _: 1
                        })
                      ]),
                      append: _withCtx(() => [
                        _createVNode(_component_q_btn, {
                          flat: "",
                          dense: "",
                          "no-caps": "",
                          size: "sm",
                          icon: "event_repeat",
                          round: !_ctx.periodText,
                          label: _ctx.periodText || undefined,
                          color: _ctx.periodText ? 'primary' : 'grey-6'
                        }, {
                          default: _withCtx(() => [
                            _createVNode(_component_q_tooltip, null, {
                              default: _withCtx(() => [
                                _createTextVNode("Period")
                              ]),
                              _: 1
                            }),
                            _createVNode(_component_q_menu, { class: "m-date-period-options q-pa-sm" }, {
                              default: _withCtx(() => [
                                _createElementVNode("div", { class: "column q-gutter-y-xs" }, [
                                  _createVNode(_component_q_select, {
                                    dense: "",
                                    outlined: "",
                                    "options-dense": "",
                                    "emit-value": "",
                                    "map-options": "",
                                    "hide-bottom-space": "",
                                    modelValue: _ctx.fields[_ctx.name+'_period'],
                                    "onUpdate:modelValue": $event => ((_ctx.fields[_ctx.name+'_period']) = $event),
                                    name: _ctx.name+'_period',
                                    "bg-color": _ctx.changed(_ctx.name+'_period'),
                                    "stack-label": "",
                                    label: "Period",
                                    options: _ctx.datePeriods,
                                    form: _ctx.form,
                                    behavior: "menu"
                                  }, null, 8, ["modelValue", "onUpdate:modelValue", "name", "bg-color", "options", "form"]),
                                  _createVNode(_component_q_select, {
                                    dense: "",
                                    outlined: "",
                                    "options-dense": "",
                                    "emit-value": "",
                                    "map-options": "",
                                    "hide-bottom-space": "",
                                    modelValue: _ctx.fields[_ctx.name+'_poffset'],
                                    "onUpdate:modelValue": $event => ((_ctx.fields[_ctx.name+'_poffset']) = $event),
                                    name: _ctx.name+'_poffset',
                                    "bg-color": _ctx.changed(_ctx.name+'_poffset'),
                                    "stack-label": "",
                                    label: "Offset",
                                    options: _ctx.dateOffsets,
                                    form: _ctx.form,
                                    behavior: "menu"
                                  }, null, 8, ["modelValue", "onUpdate:modelValue", "name", "bg-color", "options", "form"])
                                ])
                              ]),
                              _: 1
                            })
                          ]),
                          _: 1
                        }, 8, ["round", "label", "color"])
                      ]),
                      after: _withCtx(() => [
                        _createVNode(_component_q_btn, {
                          flat: "",
                          round: "",
                          dense: "",
                          size: "sm",
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
                          flat: "",
                          round: "",
                          dense: "",
                          size: "sm",
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
        "m-date-time": { hash: "94ef7446", make: function (Vue) {
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
                "hide-bottom-space": "",
                "no-error-icon": "",
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
                          size: "xs",
                          class: "cursor-pointer"
                        }, {
                          default: _withCtx(() => [
                            _createVNode(_component_q_popup_proxy, {
                              ref: "qDateProxy",
                              cover: "",
                              "transition-show": "scale",
                              "transition-hide": "scale"
                            }, {
                              default: _withCtx(() => [
                                _createVNode(_component_q_date, {
                                  "model-value": _ctx.local,
                                  "onUpdate:modelValue": _ctx.onDate,
                                  mask: _ctx.formatVal,
                                  "today-btn": ""
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
                          size: "xs",
                          class: "cursor-pointer"
                        }, {
                          default: _withCtx(() => [
                            _createVNode(_component_q_popup_proxy, {
                              ref: "qTimeProxy",
                              cover: "",
                              "transition-show": "scale",
                              "transition-hide": "scale"
                            }, {
                              default: _withCtx(() => [
                                _createVNode(_component_q_time, {
                                  "model-value": _ctx.local,
                                  "onUpdate:modelValue": _ctx.onTime,
                                  mask: _ctx.formatVal,
                                  format24h: "",
                                  "now-btn": ""
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
        "m-dialog": { hash: "353c53eb", make: function (Vue) {
            const { toDisplayString: _toDisplayString, createElementVNode: _createElementVNode, resolveComponent: _resolveComponent, createVNode: _createVNode, resolveDirective: _resolveDirective, withDirectives: _withDirectives, normalizeStyle: _normalizeStyle, withCtx: _withCtx, renderSlot: _renderSlot, openBlock: _openBlock, createBlock: _createBlock } = Vue

            return function render(_ctx, _cache) {
              const _component_q_space = _resolveComponent("q-space")
              const _component_q_btn = _resolveComponent("q-btn")
              const _component_q_card_section = _resolveComponent("q-card-section")
              const _component_q_separator = _resolveComponent("q-separator")
              const _component_q_card = _resolveComponent("q-card")
              const _component_q_dialog = _resolveComponent("q-dialog")
              const _directive_close_popup = _resolveDirective("close-popup")

              return (_openBlock(), _createBlock(_component_q_dialog, {
                "model-value": _ctx.modelValue,
                "onUpdate:modelValue": $event => (_ctx.$emit('update:modelValue', $event)),
                id: _ctx.id,
                onShow: _ctx.onShow,
                onHide: _ctx.onHide,
                maximized: _ctx.$q.screen.lt.sm
              }, {
                default: _withCtx(() => [
                  _createVNode(_component_q_card, {
                    ref: "dialogCard",
                    flat: "",
                    bordered: "",
                    style: _normalizeStyle({width:((_ctx.width||760)+'px'),'max-width':(_ctx.$q.screen.lt.sm?'100vw':'90vw')})
                  }, {
                    default: _withCtx(() => [
                      _createVNode(_component_q_card_section, {
                        ref: "dialogHeader",
                        class: "row items-center no-wrap q-py-xs q-pr-xs non-selectable",
                        style: _normalizeStyle({cursor:(_ctx.draggable?'move':'default')})
                      }, {
                        default: _withCtx(() => [
                          _createElementVNode("div", { class: "text-subtitle1 text-weight-medium ellipsis" }, _toDisplayString(_ctx.title), 1),
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
                      _createVNode(_component_q_card_section, {
                        ref: "dialogBody",
                        class: "scroll",
                        style: {"max-height":"80vh"}
                      }, {
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
        "m-drop-down": { hash: "461ba4ae", make: function (Vue) {
            const { toDisplayString: _toDisplayString, createTextVNode: _createTextVNode, resolveComponent: _resolveComponent, withCtx: _withCtx, openBlock: _openBlock, createBlock: _createBlock, createCommentVNode: _createCommentVNode, createVNode: _createVNode, renderSlot: _renderSlot, mergeProps: _mergeProps, createSlots: _createSlots } = Vue

            return function render(_ctx, _cache) {
              const _component_q_tooltip = _resolveComponent("q-tooltip")
              const _component_q_item_section = _resolveComponent("q-item-section")
              const _component_q_item = _resolveComponent("q-item")
              const _component_q_select = _resolveComponent("q-select")

              return (_openBlock(), _createBlock(_component_q_select, _mergeProps({
                ref: "qSelect",
                "model-value": _ctx.modelValue,
                "onUpdate:modelValue": _ctx.handleInput,
                dense: "",
                outlined: "",
                "options-dense": "",
                "use-input": "",
                "use-chips": _ctx.multiple,
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
                "hide-bottom-space": "",
                "no-error-icon": "",
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
                (_ctx.$slots.after)
                  ? {
                      name: "after",
                      fn: _withCtx(() => [
                        _renderSlot(_ctx.$slots, "after")
                      ]),
                      key: "0"
                    }
                  : undefined
              ]), 1040, ["model-value", "onUpdate:modelValue", "use-chips", "hide-selected", "name", "for", "form", "input-debounce", "onFilter", "clearable", "disable", "multiple", "emit-value", "new-value-mode", "rules", "label", "loading", "bg-color", "options"]))
            }
        } },
        "m-dynamic-container": { hash: "70ca5338", make: function (Vue) {
            const { resolveDynamicComponent: _resolveDynamicComponent, openBlock: _openBlock, createBlock: _createBlock } = Vue

            return function render(_ctx, _cache) {
              return (_openBlock(), _createBlock(_resolveDynamicComponent(_ctx.curComponent), { ref: "screen" }, null, 512))
            }
        } },
        "m-dynamic-dialog": { hash: "9c4723b0", make: function (Vue) {
            const { resolveComponent: _resolveComponent, normalizeClass: _normalizeClass, createVNode: _createVNode, resolveDynamicComponent: _resolveDynamicComponent, openBlock: _openBlock, createBlock: _createBlock, withCtx: _withCtx, createElementBlock: _createElementBlock } = Vue

            return function render(_ctx, _cache) {
              const _component_q_btn = _resolveComponent("q-btn")
              const _component_m_dialog = _resolveComponent("m-dialog")

              return (_openBlock(), _createElementBlock("span", null, [
                _createVNode(_component_q_btn, {
                  dense: "",
                  unelevated: "",
                  "no-caps": "",
                  icon: _ctx.buttonIcon,
                  label: _ctx.buttonText,
                  class: _normalizeClass(_ctx.moqui.tonalClass(_ctx.color, _ctx.buttonClass)),
                  onClick: $event => (_ctx.isShown = true)
                }, null, 8, ["icon", "label", "class", "onClick"]),
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
        "m-editable": { hash: "49667c8e", make: function (Vue) {
            const { toDisplayString: _toDisplayString, openBlock: _openBlock, createElementBlock: _createElementBlock, createCommentVNode: _createCommentVNode, resolveComponent: _resolveComponent, createBlock: _createBlock, createVNode: _createVNode, withKeys: _withKeys, withCtx: _withCtx, resolveDynamicComponent: _resolveDynamicComponent, normalizeClass: _normalizeClass } = Vue

            return function render(_ctx, _cache) {
              const _component_q_select = _resolveComponent("q-select")
              const _component_q_date = _resolveComponent("q-date")
              const _component_q_time = _resolveComponent("q-time")
              const _component_q_input = _resolveComponent("q-input")
              const _component_q_popup_edit = _resolveComponent("q-popup-edit")

              return (_openBlock(), _createBlock(_resolveDynamicComponent(_ctx.labelType), {
                id: _ctx.id,
                class: _normalizeClass(["editable-label cursor-pointer", {'m-editable-empty': _ctx.isEmpty}]),
                title: _ctx.tooltip
              }, {
                default: _withCtx(() => [
                  (_ctx.isEmpty)
                    ? (_openBlock(), _createElementBlock("span", {
                        key: 0,
                        class: "m-muted m-editable-placeholder",
                        textContent: _toDisplayString(_ctx.placeholder || _ctx.tooltip)
                      }, null, 8, ["textContent"]))
                    : (_openBlock(), _createElementBlock("span", {
                        key: 1,
                        innerHTML: _ctx.shown
                      }, null, 8, ["innerHTML"])),
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
                      (_ctx.widgetType === 'select')
                        ? (_openBlock(), _createBlock(_component_q_select, {
                            key: 0,
                            modelValue: scope.value,
                            "onUpdate:modelValue": $event => ((scope.value) = $event),
                            options: _ctx.selectOptions,
                            "emit-value": "",
                            "map-options": "",
                            dense: "",
                            outlined: "",
                            "options-dense": "",
                            autofocus: "",
                            loading: _ctx.saving || _ctx.loadingOptions,
                            class: "m-editable-select"
                          }, null, 8, ["modelValue", "onUpdate:modelValue", "options", "loading"]))
                        : (_ctx.isDate)
                          ? (_openBlock(), _createElementBlock("div", {
                              key: 1,
                              class: "row no-wrap q-gutter-sm m-editable-date"
                            }, [
                              _createVNode(_component_q_date, {
                                modelValue: scope.value,
                                "onUpdate:modelValue": $event => ((scope.value) = $event),
                                mask: _ctx.dateMask,
                                minimal: "",
                                flat: ""
                              }, null, 8, ["modelValue", "onUpdate:modelValue", "mask"]),
                              (_ctx.widgetType === 'date-time')
                                ? (_openBlock(), _createBlock(_component_q_time, {
                                    key: 0,
                                    modelValue: scope.value,
                                    "onUpdate:modelValue": $event => ((scope.value) = $event),
                                    mask: _ctx.dateMask,
                                    format24h: "",
                                    flat: ""
                                  }, null, 8, ["modelValue", "onUpdate:modelValue", "mask"]))
                                : _createCommentVNode("", true)
                            ]))
                          : (_openBlock(), _createBlock(_component_q_input, {
                              key: 2,
                              modelValue: scope.value,
                              "onUpdate:modelValue": $event => ((scope.value) = $event),
                              dense: "",
                              autofocus: "",
                              type: _ctx.widgetType === 'textarea' ? 'textarea' : 'text',
                              loading: _ctx.saving,
                              onKeyup: _withKeys($event => (_ctx.widgetType === 'textarea' ? null : scope.set()), ["enter"])
                            }, null, 8, ["modelValue", "onUpdate:modelValue", "type", "loading", "onKeyup"]))
                    ]),
                    _: 1
                  }, 8, ["modelValue", "onUpdate:modelValue", "label-set", "label-cancel", "onBeforeShow", "onSave"])
                ]),
                _: 1
              }, 8, ["id", "class", "title"]))
            }
        } },
        "m-elapsed": { hash: "86a2aa", make: function (Vue) {
            const { toDisplayString: _toDisplayString, openBlock: _openBlock, createElementBlock: _createElementBlock } = Vue

            return function render(_ctx, _cache) {
              return (_openBlock(), _createElementBlock("span", { class: "m-elapsed" }, _toDisplayString(_ctx.text), 1))
            }
        } },
        "m-field": { hash: "15d75803", make: function (Vue) {
            const { renderSlot: _renderSlot, resolveComponent: _resolveComponent, mergeProps: _mergeProps, withCtx: _withCtx, openBlock: _openBlock, createBlock: _createBlock } = Vue

            return function render(_ctx, _cache) {
              const _component_q_field = _resolveComponent("q-field")

              return (_openBlock(), _createBlock(_component_q_field, _mergeProps({
                dense: "",
                outlined: !_ctx.borderless,
                borderless: _ctx.borderless,
                "stack-label": "",
                "hide-bottom-space": "",
                label: _ctx.label,
                "bg-color": _ctx.bgColor,
                disable: _ctx.disable
              }, _ctx.$attrs, { "model-value": 1 }), {
                control: _withCtx(() => [
                  _renderSlot(_ctx.$slots, "default")
                ]),
                _: 3
              }, 16, ["outlined", "borderless", "label", "bg-color", "disable"]))
            }
        } },
        "m-file": { hash: "4423bd8", make: function (Vue) {
            const { resolveComponent: _resolveComponent, createVNode: _createVNode, toDisplayString: _toDisplayString, createTextVNode: _createTextVNode, withCtx: _withCtx, openBlock: _openBlock, createBlock: _createBlock, createCommentVNode: _createCommentVNode, mergeProps: _mergeProps } = Vue

            return function render(_ctx, _cache) {
              const _component_q_icon = _resolveComponent("q-icon")
              const _component_q_tooltip = _resolveComponent("q-tooltip")
              const _component_q_file = _resolveComponent("q-file")

              return (_openBlock(), _createBlock(_component_q_file, _mergeProps({
                dense: "",
                outlined: "",
                "stack-label": "",
                "hide-bottom-space": "",
                "no-error-icon": "",
                "use-chips": _ctx.multiple,
                counter: _ctx.multiple,
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
                append: _withCtx(() => [
                  _createVNode(_component_q_icon, {
                    name: "attach_file",
                    size: "xs"
                  })
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
              }, 16, ["use-chips", "counter", "label", "model-value", "onUpdate:modelValue", "name", "for", "size", "multiple", "accept", "form", "disable", "bg-color"]))
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
        "m-form-column-config": { hash: "ab938ef1", make: function (Vue) {
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
                      dense: "",
                      class: "q-mb-sm rounded-borders"
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
                                        flat: "",
                                        round: "",
                                        dense: "",
                                        size: "sm",
                                        icon: "close",
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
                                        flat: "",
                                        "no-caps": "",
                                        label: "Display",
                                        class: "m-btn-tonal"
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
                                            size: "sm",
                                            icon: "north",
                                            onClick: $event => (_ctx.moveToCol(columnIdx, fieldIdx, columnIdx-1))
                                          }, null, 8, ["disabled", "onClick"]),
                                          _createVNode(_component_q_btn, {
                                            disabled: fieldIdx === 0,
                                            dense: "",
                                            flat: "",
                                            size: "sm",
                                            icon: "expand_less",
                                            onClick: $event => (_ctx.moveInCol(columnIdx, fieldIdx, fieldIdx-1))
                                          }, null, 8, ["disabled", "onClick"]),
                                          _createVNode(_component_q_btn, {
                                            disabled: (fieldIdx + 1) === column.children.length,
                                            dense: "",
                                            flat: "",
                                            size: "sm",
                                            icon: "expand_more",
                                            onClick: $event => (_ctx.moveInCol(columnIdx, fieldIdx, fieldIdx+1))
                                          }, null, 8, ["disabled", "onClick"]),
                                          _createVNode(_component_q_btn, {
                                            dense: "",
                                            flat: "",
                                            size: "sm",
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
                      unelevated: "",
                      "no-caps": "",
                      color: "primary",
                      onClick: _withModifiers($event => (_ctx.saveColumns()), ["prevent"]),
                      label: "Save Changes"
                    }, null, 8, ["onClick"]),
                    _createVNode(_component_q_btn, {
                      dense: "",
                      flat: "",
                      "no-caps": "",
                      onClick: _withModifiers($event => (_ctx.resetColumns()), ["prevent"]),
                      label: "Undo Changes"
                    }, null, 8, ["onClick"]),
                    _createVNode(_component_q_btn, {
                      dense: "",
                      flat: "",
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
        "m-form-go-page": { hash: "1b62080b", make: function (Vue) {
            const { resolveComponent: _resolveComponent, createVNode: _createVNode, withCtx: _withCtx, withModifiers: _withModifiers, openBlock: _openBlock, createBlock: _createBlock, createCommentVNode: _createCommentVNode } = Vue

            return function render(_ctx, _cache) {
              const _component_q_btn = _resolveComponent("q-btn")
              const _component_q_input = _resolveComponent("q-input")
              const _component_q_form = _resolveComponent("q-form")

              return (!_ctx.formList || (_ctx.formList.paginate && _ctx.formList.paginate.pageMaxIndex > 4))
                ? (_openBlock(), _createBlock(_component_q_form, {
                    key: 0,
                    onSubmit: _withModifiers(_ctx.goPage, ["prevent"]),
                    class: "q-mx-xs"
                  }, {
                    default: _withCtx(() => [
                      _createVNode(_component_q_input, {
                        dense: "",
                        outlined: "",
                        "hide-bottom-space": "",
                        modelValue: _ctx.pageIndex,
                        "onUpdate:modelValue": $event => ((_ctx.pageIndex) = $event),
                        type: "text",
                        name: "pageIndex",
                        placeholder: "Page #",
                        style: {"width":"96px"},
                        "input-class": "text-center",
                        rules: [val => /^\d*$/.test(val) || 'digits only', val => !val || ((_ctx.formList && +val <= _ctx.formList.paginate.pageMaxIndex + 1) || (_ctx.maxIndex && +val <= _ctx.maxIndex + 1)) || 'higher than max']
                      }, {
                        append: _withCtx(() => [
                          _createVNode(_component_q_btn, {
                            flat: "",
                            round: "",
                            dense: "",
                            size: "sm",
                            type: "submit",
                            icon: "arrow_forward"
                          })
                        ]),
                        _: 1
                      }, 8, ["modelValue", "onUpdate:modelValue", "rules"])
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
        "m-form-list": { hash: "ae8730d0", make: function (Vue) {
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
                          flat: "",
                          "no-caps": "",
                          icon: "download",
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
                                  class: "th m-sel"
                                }, [
                                  _createVNode(_component_q_checkbox, {
                                    dense: "",
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
                                  class: "td m-sel"
                                }, [
                                  _createVNode(_component_q_checkbox, {
                                    dense: "",
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
                  ], 8, ["id"]),
                  (_ctx.paginate && _ctx.paginate.pageMaxIndex > 0)
                    ? (_openBlock(), _createElementBlock("div", {
                        key: 0,
                        class: "form-list-nav-row row items-center"
                      }, [
                        _createVNode(_component_q_space),
                        _createVNode(_component_m_form_go_page, {
                          "id-val": _ctx.idVal,
                          "form-list": this
                        }, null, 8, ["id-val"]),
                        _createVNode(_component_m_form_paginate, {
                          paginate: _ctx.paginate,
                          "form-list": this
                        }, null, 8, ["paginate"])
                      ]))
                    : _createCommentVNode("", true)
                ], 2)
              ]))
            }
        } },
        "m-form-paginate": { hash: "e892c70f", make: function (Vue) {
            const { toDisplayString: _toDisplayString, createElementVNode: _createElementVNode, resolveComponent: _resolveComponent, openBlock: _openBlock, createBlock: _createBlock, createCommentVNode: _createCommentVNode, createElementBlock: _createElementBlock } = Vue

            return function render(_ctx, _cache) {
              const _component_q_pagination = _resolveComponent("q-pagination")

              return (_ctx.paginate && _ctx.paginate.count > 1)
                ? (_openBlock(), _createElementBlock("div", {
                    key: 0,
                    class: "row no-wrap items-center"
                  }, [
                    _createElementVNode("span", { class: "text-caption text-grey-7 q-mx-sm" }, _toDisplayString(_ctx.paginate.pageRangeLow) + "-" + _toDisplayString(_ctx.paginate.pageRangeHigh) + " / " + _toDisplayString(_ctx.paginate.count), 1),
                    (_ctx.paginate.pageMaxIndex > 0)
                      ? (_openBlock(), _createBlock(_component_q_pagination, {
                          key: 0,
                          "model-value": _ctx.paginate.pageIndex + 1,
                          "onUpdate:modelValue": $event => (_ctx.setIndex($event - 1)),
                          max: _ctx.paginate.pageMaxIndex + 1,
                          "max-pages": 7,
                          "direction-links": "",
                          "boundary-links": "",
                          size: "sm",
                          flat: "",
                          "active-design": "unelevated",
                          color: "grey-8",
                          "active-color": "primary"
                        }, null, 8, ["model-value", "onUpdate:modelValue", "max"]))
                      : _createCommentVNode("", true)
                  ]))
                : _createCommentVNode("", true)
            }
        } },
        "m-html-view": { hash: "b2da5b44", make: function (Vue) {
            const { resolveComponent: _resolveComponent, createVNode: _createVNode, toDisplayString: _toDisplayString, createElementVNode: _createElementVNode, openBlock: _openBlock, createElementBlock: _createElementBlock, createCommentVNode: _createCommentVNode, normalizeClass: _normalizeClass, normalizeStyle: _normalizeStyle } = Vue

            return function render(_ctx, _cache) {
              const _component_q_icon = _resolveComponent("q-icon")
              const _component_q_btn = _resolveComponent("q-btn")

              return (_openBlock(), _createElementBlock("div", {
                class: "m-html-view",
                "data-state": _ctx.state,
                "data-blocked": _ctx.blocked
              }, [
                (_ctx.blocked && !_ctx.remoteAllowed && _ctx.imagesBar)
                  ? (_openBlock(), _createElementBlock("div", {
                      key: 0,
                      class: "m-html-view-images row items-center no-wrap"
                    }, [
                      _createVNode(_component_q_icon, {
                        name: "sym_o_hide_image",
                        size: "xs",
                        class: "q-mr-sm"
                      }),
                      _createElementVNode("span", { class: "col m-muted" }, _toDisplayString(_ctx.t("Images from this sender are hidden")), 1),
                      _createVNode(_component_q_btn, {
                        flat: "",
                        dense: "",
                        "no-caps": "",
                        color: "primary",
                        class: "m-html-view-show",
                        label: _ctx.t('Show images'),
                        onClick: _ctx.showImages
                      }, null, 8, ["label", "onClick"])
                    ]))
                  : _createCommentVNode("", true),
                _createElementVNode("iframe", {
                  ref: "frame",
                  class: _normalizeClass(["m-html-view-frame", {'m-html-view-card': _ctx.designed && _ctx.isDark}]),
                  title: _ctx.title,
                  sandbox: "allow-same-origin allow-popups allow-popups-to-escape-sandbox",
                  referrerpolicy: "no-referrer",
                  srcdoc: _ctx.srcdoc,
                  style: _normalizeStyle(_ctx.frameStyle),
                  onLoad: _ctx.onLoad
                }, null, 46, ["title", "srcdoc", "onLoad"])
              ], 8, ["data-state", "data-blocked"]))
            }
        } },
        "m-link": { hash: "449c1400", make: function (Vue) {
            const { renderSlot: _renderSlot, openBlock: _openBlock, createElementBlock: _createElementBlock } = Vue

            return function render(_ctx, _cache) {
              return (_openBlock(), _createElementBlock("a", {
                href: _ctx.linkHref,
                onClick: _ctx.go,
                class: "q-link"
              }, [
                _renderSlot(_ctx.$slots, "default")
              ], 8, ["href", "onClick"]))
            }
        } },
        "m-list-tree": { hash: "f5d4492", make: function (Vue) {
            const { renderList: _renderList, Fragment: _Fragment, openBlock: _openBlock, createElementBlock: _createElementBlock, toDisplayString: _toDisplayString, createTextVNode: _createTextVNode, resolveComponent: _resolveComponent, withCtx: _withCtx, createVNode: _createVNode, resolveDirective: _resolveDirective, createBlock: _createBlock, withDirectives: _withDirectives, createElementVNode: _createElementVNode, normalizeClass: _normalizeClass, renderSlot: _renderSlot, createCommentVNode: _createCommentVNode, normalizeStyle: _normalizeStyle } = Vue

            return function render(_ctx, _cache) {
              const _component_q_item_section = _resolveComponent("q-item-section")
              const _component_q_item = _resolveComponent("q-item")
              const _component_q_list = _resolveComponent("q-list")
              const _component_q_btn_dropdown = _resolveComponent("q-btn-dropdown")
              const _component_m_tree_lead = _resolveComponent("m-tree-lead")
              const _component_q_skeleton = _resolveComponent("q-skeleton")
              const _component_q_btn = _resolveComponent("q-btn")
              const _directive_close_popup = _resolveDirective("close-popup")

              return (_openBlock(), _createElementBlock("div", {
                class: _normalizeClass(["m-list-tree", {'m-list-tree--dragging': !!_ctx.dragKey}])
              }, [
                _createElementVNode("div", { class: "m-tree-bar row items-center no-wrap" }, [
                  _createVNode(_component_q_btn_dropdown, {
                    flat: "",
                    dense: "",
                    "no-caps": "",
                    icon: "sym_o_unfold_more",
                    class: "m-tree-expand",
                    label: _ctx.labels.expand
                  }, {
                    default: _withCtx(() => [
                      _createVNode(_component_q_list, { dense: "" }, {
                        default: _withCtx(() => [
                          (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.levelOptions, (option) => {
                            return _withDirectives((_openBlock(), _createBlock(_component_q_item, {
                              key: option.value,
                              clickable: "",
                              active: _ctx.level === option.value,
                              "data-level": option.value,
                              onClick: $event => (_ctx.expandTo(option.value))
                            }, {
                              default: _withCtx(() => [
                                _createVNode(_component_q_item_section, null, {
                                  default: _withCtx(() => [
                                    _createTextVNode(_toDisplayString(option.label), 1)
                                  ]),
                                  _: 2
                                }, 1024)
                              ]),
                              _: 2
                            }, 1032, ["active", "data-level", "onClick"])), [
                              [_directive_close_popup]
                            ])
                          }), 128))
                        ]),
                        _: 1
                      })
                    ]),
                    _: 1
                  }, 8, ["label"])
                ]),
                _createElementVNode("div", {
                  class: _normalizeClass(["q-my-sm q-table__container q-table__card q-table--horizontal-separator q-table--dense q-table--flat", {'q-table--dark': _ctx.$q.dark.isActive, 'q-table__card--dark': _ctx.$q.dark.isActive, 'q-dark': _ctx.$q.dark.isActive}])
                }, [
                  _createElementVNode("div", {
                    class: "table q-table m-tree-table",
                    role: "treegrid",
                    "aria-label": _ctx.labels.tree
                  }, [
                    _createElementVNode("div", { class: "thead" }, [
                      _createElementVNode("div", {
                        class: "tr",
                        role: "row"
                      }, [
                        (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.tree.columns, (column, c) => {
                          return (_openBlock(), _createElementBlock("div", {
                            key: 'h' + c,
                            class: _normalizeClass(["th", column.align === 'right' ? 'text-right' : 'text-left']),
                            role: "columnheader"
                          }, _toDisplayString(column.label), 3))
                        }), 128)),
                        (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.rollups, (rollup) => {
                          return (_openBlock(), _createElementBlock("div", {
                            key: 'r' + rollup.name,
                            class: "th text-right m-tree-rollup",
                            role: "columnheader"
                          }, _toDisplayString(rollup.label), 1))
                        }), 128))
                      ])
                    ]),
                    _createElementVNode("div", {
                      ref: "body",
                      class: "tbody",
                      onKeydown: _ctx.onKey
                    }, [
                      (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.rows, (entry) => {
                        return (_openBlock(), _createElementBlock(_Fragment, {
                          key: entry.id
                        }, [
                          (entry.kind === 'row')
                            ? (_openBlock(), _createElementBlock("div", {
                                key: 0,
                                class: _normalizeClass(["tr m-tree-row", entry.classes]),
                                role: "row",
                                "data-tree-key": entry.key,
                                tabindex: entry.key === _ctx.focusShown ? 0 : -1,
                                "aria-level": entry.level,
                                "aria-setsize": entry.setsize,
                                "aria-posinset": entry.posinset,
                                "aria-expanded": entry.parent ? String(entry.open) : null,
                                "aria-selected": entry.selected ? 'true' : null,
                                draggable: _ctx.canMove && !entry.muted && _ctx.armed === entry.key ? 'true' : null,
                                onClick: $event => (_ctx.onRowClick($event, entry)),
                                onFocus: $event => (_ctx.focusKey = entry.key),
                                onPointerdown: $event => (_ctx.arm($event, entry)),
                                onDragstart: $event => (_ctx.onDragStart($event, entry)),
                                onDragover: $event => (_ctx.onDragOver($event, entry)),
                                onDrop: $event => (_ctx.onDrop($event, entry)),
                                onDragend: _ctx.onDragEnd
                              }, [
                                _renderSlot(_ctx.$slots, 'tree_' + entry.index, {
                                  t: entry.t,
                                  row: entry.row
                                }, () => [
                                  _renderSlot(_ctx.$slots, "tree", {
                                    t: entry.t,
                                    row: entry.row
                                  }, () => [
                                    (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.tree.columns, (column, c) => {
                                      return (_openBlock(), _createElementBlock("div", {
                                        key: c,
                                        class: _normalizeClass(["td", column.align === 'right' ? 'text-right' : null])
                                      }, [
                                        (c === 0)
                                          ? (_openBlock(), _createElementBlock("div", {
                                              key: 0,
                                              class: "m-tree-cell row no-wrap items-center"
                                            }, [
                                              _createVNode(_component_m_tree_lead, {
                                                t: entry.t
                                              }, null, 8, ["t"]),
                                              _createElementVNode("div", { class: "m-tree-label" }, _toDisplayString(_ctx.cellText(entry.row, column)), 1)
                                            ]))
                                          : (_openBlock(), _createElementBlock(_Fragment, { key: 1 }, [
                                              _createTextVNode(_toDisplayString(_ctx.cellText(entry.row, column)), 1)
                                            ], 64))
                                      ], 2))
                                    }), 128))
                                  ])
                                ]),
                                (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.rollups, (rollup) => {
                                  return (_openBlock(), _createElementBlock("div", {
                                    key: rollup.name,
                                    class: _normalizeClass(["td text-right m-tree-rollup", {'m-muted': !entry.parent}])
                                  }, _toDisplayString(_ctx.rollupText(entry.row, rollup.name)), 3))
                                }), 128))
                              ], 42, ["data-tree-key", "tabindex", "aria-level", "aria-setsize", "aria-posinset", "aria-expanded", "aria-selected", "draggable", "onClick", "onFocus", "onPointerdown", "onDragstart", "onDragover", "onDrop", "onDragend"]))
                            : (_openBlock(), _createElementBlock("div", {
                                key: 1,
                                class: "tr m-tree-extra",
                                "data-extra": entry.kind
                              }, [
                                _createElementVNode("div", { class: "td" }, [
                                  _createElementVNode("div", {
                                    class: "m-tree-cell row no-wrap items-center",
                                    style: _normalizeStyle({paddingLeft: (_ctx.indentOf(entry.level).px + 24) + 'px'})
                                  }, [
                                    (entry.kind === 'loading')
                                      ? (_openBlock(), _createBlock(_component_q_skeleton, {
                                          key: 0,
                                          type: "text",
                                          width: "160px",
                                          height: "20px"
                                        }))
                                      : (entry.kind === 'error')
                                        ? (_openBlock(), _createElementBlock(_Fragment, { key: 1 }, [
                                            _createElementVNode("span", { class: "text-negative" }, _toDisplayString(_ctx.labels.failed), 1),
                                            _createVNode(_component_q_btn, {
                                              flat: "",
                                              dense: "",
                                              "no-caps": "",
                                              color: "primary",
                                              class: "q-ml-sm m-tree-retry",
                                              label: _ctx.labels.retry,
                                              onClick: $event => (_ctx.fetchLevel(entry.key))
                                            }, null, 8, ["label", "onClick"])
                                          ], 64))
                                        : (_openBlock(), _createBlock(_component_q_btn, {
                                            key: 2,
                                            flat: "",
                                            dense: "",
                                            "no-caps": "",
                                            color: "primary",
                                            class: "m-tree-more",
                                            label: _ctx.labels.more,
                                            onClick: $event => (_ctx.more[entry.key] = _ctx.shownOf(entry.key) + 200)
                                          }, null, 8, ["label", "onClick"]))
                                  ], 4)
                                ]),
                                (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.fillers, (n) => {
                                  return (_openBlock(), _createElementBlock("div", {
                                    key: n,
                                    class: "td"
                                  }))
                                }), 128))
                              ], 8, ["data-extra"]))
                        ], 64))
                      }), 128)),
                      (!_ctx.rows.length)
                        ? (_openBlock(), _createElementBlock("div", {
                            key: 0,
                            class: "tr"
                          }, [
                            _createElementVNode("div", { class: "td m-muted" }, _toDisplayString(_ctx.labels.empty), 1),
                            (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.fillers, (n) => {
                              return (_openBlock(), _createElementBlock("div", {
                                key: n,
                                class: "td"
                              }))
                            }), 128))
                          ]))
                        : _createCommentVNode("", true),
                      (_ctx.rollups.length && _ctx.rows.length)
                        ? (_openBlock(), _createElementBlock("div", {
                            key: 1,
                            class: "tr m-tree-total text-weight-bold"
                          }, [
                            _createElementVNode("div", { class: "td" }, _toDisplayString(_ctx.totalLabel), 1),
                            (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.tree.columns.length - 1, (n) => {
                              return (_openBlock(), _createElementBlock("div", {
                                key: 'f' + n,
                                class: "td"
                              }))
                            }), 128)),
                            (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.rollups, (rollup) => {
                              return (_openBlock(), _createElementBlock("div", {
                                key: rollup.name,
                                class: "td text-right m-tree-rollup"
                              }, _toDisplayString(_ctx.totalText(rollup.name)), 1))
                            }), 128))
                          ]))
                        : _createCommentVNode("", true)
                    ], 40, ["onKeydown"])
                  ], 8, ["aria-label"])
                ], 2)
              ], 2))
            }
        } },
        "m-list-views": { hash: "e64b3065", make: function (Vue) {
            const { renderList: _renderList, Fragment: _Fragment, openBlock: _openBlock, createElementBlock: _createElementBlock, toDisplayString: _toDisplayString, createTextVNode: _createTextVNode, resolveComponent: _resolveComponent, withCtx: _withCtx, createVNode: _createVNode, resolveDirective: _resolveDirective, createBlock: _createBlock, withDirectives: _withDirectives, createCommentVNode: _createCommentVNode, normalizeClass: _normalizeClass, renderSlot: _renderSlot, createElementVNode: _createElementVNode, normalizeProps: _normalizeProps, guardReactiveProps: _guardReactiveProps, createSlots: _createSlots, withModifiers: _withModifiers, normalizeStyle: _normalizeStyle } = Vue

            return function render(_ctx, _cache) {
              const _component_q_item_section = _resolveComponent("q-item-section")
              const _component_q_item = _resolveComponent("q-item")
              const _component_q_list = _resolveComponent("q-list")
              const _component_q_btn_dropdown = _resolveComponent("q-btn-dropdown")
              const _component_q_space = _resolveComponent("q-space")
              const _component_q_tooltip = _resolveComponent("q-tooltip")
              const _component_q_btn = _resolveComponent("q-btn")
              const _component_q_btn_toggle = _resolveComponent("q-btn-toggle")
              const _component_m_list_tree = _resolveComponent("m-list-tree")
              const _component_q_badge = _resolveComponent("q-badge")
              const _component_q_card = _resolveComponent("q-card")
              const _component_q_separator = _resolveComponent("q-separator")
              const _component_q_item_label = _resolveComponent("q-item-label")
              const _component_q_menu = _resolveComponent("q-menu")
              const _component_m_dynamic_container = _resolveComponent("m-dynamic-container")
              const _component_m_text_line = _resolveComponent("m-text-line")
              const _component_m_date_time = _resolveComponent("m-date-time")
              const _component_m_form = _resolveComponent("m-form")
              const _component_m_dialog = _resolveComponent("m-dialog")
              const _directive_close_popup = _resolveDirective("close-popup")

              return (_openBlock(), _createElementBlock("div", {
                class: _normalizeClass(["m-list-views", ['m-list-views--' + _ctx.current, {'m-list-views--peek': _ctx.peekShown}]]),
                id: _ctx.id,
                "data-view": _ctx.current
              }, [
                (_ctx.viewOptions.length > 1 || _ctx.peekAvailable)
                  ? (_openBlock(), _createElementBlock("div", {
                      key: 0,
                      class: "m-list-views-bar row items-center no-wrap"
                    }, [
                      (_ctx.current === 'board' && _ctx.groupOptions.length > 1)
                        ? (_openBlock(), _createBlock(_component_q_btn_dropdown, {
                            key: 0,
                            flat: "",
                            dense: "",
                            "no-caps": "",
                            class: "m-board-group",
                            label: _ctx.labels.groupBy + ': ' + _ctx.groupLabel
                          }, {
                            default: _withCtx(() => [
                              _createVNode(_component_q_list, { dense: "" }, {
                                default: _withCtx(() => [
                                  (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.groupOptions, (option) => {
                                    return _withDirectives((_openBlock(), _createBlock(_component_q_item, {
                                      key: option.field,
                                      clickable: "",
                                      active: option.field === _ctx.boardView.groupField,
                                      "data-group": option.field,
                                      onClick: $event => (_ctx.setGroup(option.field))
                                    }, {
                                      default: _withCtx(() => [
                                        _createVNode(_component_q_item_section, null, {
                                          default: _withCtx(() => [
                                            _createTextVNode(_toDisplayString(option.label), 1)
                                          ]),
                                          _: 2
                                        }, 1024)
                                      ]),
                                      _: 2
                                    }, 1032, ["active", "data-group", "onClick"])), [
                                      [_directive_close_popup]
                                    ])
                                  }), 128))
                                ]),
                                _: 1
                              })
                            ]),
                            _: 1
                          }, 8, ["label"]))
                        : _createCommentVNode("", true),
                      _createVNode(_component_q_space),
                      (_ctx.peekAvailable)
                        ? (_openBlock(), _createBlock(_component_q_btn, {
                            key: 1,
                            flat: "",
                            dense: "",
                            "no-caps": "",
                            icon: "sym_o_right_panel_open",
                            class: _normalizeClass(["m-peek-toggle q-mr-sm", {'text-primary': _ctx.peekOn}]),
                            label: _ctx.labels.peek,
                            "aria-pressed": _ctx.peekOn ? 'true' : 'false',
                            onClick: $event => (_ctx.setPeek(!_ctx.peekOn))
                          }, {
                            default: _withCtx(() => [
                              _createVNode(_component_q_tooltip, null, {
                                default: _withCtx(() => [
                                  _createTextVNode(_toDisplayString(_ctx.labels.peekTip), 1)
                                ]),
                                _: 1
                              })
                            ]),
                            _: 1
                          }, 8, ["class", "label", "aria-pressed", "onClick"]))
                        : _createCommentVNode("", true),
                      (_ctx.viewOptions.length > 1)
                        ? (_openBlock(), _createBlock(_component_q_btn_toggle, {
                            key: 2,
                            "model-value": _ctx.current,
                            "onUpdate:modelValue": _ctx.setView,
                            dense: "",
                            unelevated: "",
                            "no-caps": "",
                            "toggle-color": "primary",
                            class: "m-list-views-toggle",
                            options: _ctx.viewOptions
                          }, {
                            list: _withCtx(() => [
                              _createVNode(_component_q_tooltip, null, {
                                default: _withCtx(() => [
                                  _createTextVNode(_toDisplayString(_ctx.labels.list), 1)
                                ]),
                                _: 1
                              })
                            ]),
                            board: _withCtx(() => [
                              _createVNode(_component_q_tooltip, null, {
                                default: _withCtx(() => [
                                  _createTextVNode(_toDisplayString(_ctx.labels.board), 1)
                                ]),
                                _: 1
                              })
                            ]),
                            calendar: _withCtx(() => [
                              _createVNode(_component_q_tooltip, null, {
                                default: _withCtx(() => [
                                  _createTextVNode(_toDisplayString(_ctx.labels.calendar), 1)
                                ]),
                                _: 1
                              })
                            ]),
                            tree: _withCtx(() => [
                              _createVNode(_component_q_tooltip, null, {
                                default: _withCtx(() => [
                                  _createTextVNode(_toDisplayString(_ctx.labels.tree), 1)
                                ]),
                                _: 1
                              })
                            ]),
                            _: 1
                          }, 8, ["model-value", "onUpdate:modelValue", "options"]))
                        : _createCommentVNode("", true)
                    ]))
                  : _createCommentVNode("", true),
                _createElementVNode("div", {
                  ref: "body",
                  class: _normalizeClass(["m-list-views-body", {'row no-wrap': _ctx.peekShown}])
                }, [
                  _createElementVNode("div", {
                    class: _normalizeClass(["m-list-views-content", {'col': _ctx.peekShown}]),
                    onClick: _ctx.onPeekClick
                  }, [
                    _createElementVNode("div", { class: "m-list-views-list" }, [
                      _renderSlot(_ctx.$slots, "default")
                    ]),
                    (_ctx.current === 'tree' && _ctx.tree)
                      ? (_openBlock(), _createBlock(_component_m_list_tree, {
                          key: 0,
                          ref: "tree",
                          tree: _ctx.tree,
                          items: _ctx.items,
                          "pref-key": _ctx.prefKey,
                          find: _ctx.findOf,
                          post: _ctx.post,
                          peekable: _ctx.peekShown,
                          selected: _ctx.peekShown ? _ctx.peekKey : '',
                          "peek-field": _ctx.peek ? _ctx.peek.keyField : '',
                          onSelect: (row) => _ctx.peekSelect(_ctx.peekKeyOf(row)),
                          onMoved: _ctx.reloadRows,
                          onFocusrow: (row) => _ctx.treeFocus(_ctx.peekKeyOf(row)),
                          onKeys: (rows) => { _ctx.treeKeys = rows.map(_ctx.peekKeyOf); }
                        }, _createSlots({ _: 2 }, [
                          _renderList(_ctx.treeSlotNames(), (name) => {
                            return {
                              name: name,
                              fn: _withCtx((props) => [
                                _renderSlot(_ctx.$slots, name, _normalizeProps(_guardReactiveProps(props)))
                              ])
                            }
                          })
                        ]), 1032, ["tree", "items", "pref-key", "find", "post", "peekable", "selected", "peek-field", "onSelect", "onMoved", "onFocusrow", "onKeys"]))
                      : _createCommentVNode("", true),
                    (_ctx.current === 'board')
                      ? (_openBlock(), _createElementBlock("div", {
                          key: 1,
                          ref: "board",
                          class: _normalizeClass(["m-board row no-wrap", {'m-board--dragging': !!_ctx.dragging}])
                        }, [
                          (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.columns, (column) => {
                            return (_openBlock(), _createElementBlock("div", {
                              key: column.value,
                              class: _normalizeClass(["m-board-col column no-wrap", _ctx.dragging ? (column.value === _ctx.dragging.from ? 'm-board-col--from' : (_ctx.dragAllowed(column.value) ? 'm-board-col--target' : 'm-board-col--blocked')) : '']),
                              "data-value": column.value
                            }, [
                              _createElementVNode("div", { class: "m-board-col-head row items-center no-wrap" }, [
                                _createElementVNode("span", {
                                  class: _normalizeClass(["m-board-col-dot", 'bg-' + (column.color || 'grey-6')])
                                }, null, 2),
                                _createElementVNode("span", { class: "m-board-col-label ellipsis" }, _toDisplayString(column.label), 1),
                                _createVNode(_component_q_badge, {
                                  class: _normalizeClass(["m-board-col-count m-tonal q-ml-xs", column.color ? 'text-' + column.color : 'text-grey-8'])
                                }, {
                                  default: _withCtx(() => [
                                    _createTextVNode(_toDisplayString(column.count), 1)
                                  ]),
                                  _: 2
                                }, 1032, ["class"]),
                                _createVNode(_component_q_space),
                                (_ctx.board.sumField)
                                  ? (_openBlock(), _createElementBlock("span", {
                                      key: 0,
                                      class: "m-board-col-sum m-muted"
                                    }, _toDisplayString(_ctx.sumText(column.sum)), 1))
                                  : _createCommentVNode("", true)
                              ]),
                              _createElementVNode("div", {
                                class: "m-board-cards col",
                                "data-value": column.value
                              }, [
                                (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(column.items, (item) => {
                                  return (_openBlock(), _createElementBlock("div", {
                                    key: item.index,
                                    class: _normalizeClass(["m-board-card", {'m-board-card--selected': _ctx.peekShown && !!_ctx.peekKey && _ctx.peekKey === _ctx.peekKeyOf(item.row)}]),
                                    "data-index": item.index,
                                    onClick: $event => (_ctx.openCard($event, item))
                                  }, [
                                    _renderSlot(_ctx.$slots, 'card_' + item.index, {
                                      index: item.index,
                                      row: item.row
                                    }, () => [
                                      _renderSlot(_ctx.$slots, "card", {
                                        index: item.index,
                                        row: item.row
                                      }, () => [
                                        _createElementVNode("div", { class: "m-card-title" }, _toDisplayString(_ctx.cardTitle(item.row)), 1)
                                      ])
                                    ])
                                  ], 10, ["data-index", "onClick"]))
                                }), 128))
                              ], 8, ["data-value"]),
                              (!column.count)
                                ? (_openBlock(), _createElementBlock("div", {
                                    key: 0,
                                    class: "m-board-empty m-muted"
                                  }, _toDisplayString(_ctx.labels.noCards), 1))
                                : _createCommentVNode("", true)
                            ], 10, ["data-value"]))
                          }), 128)),
                          (!_ctx.columns.length)
                            ? (_openBlock(), _createElementBlock("div", {
                                key: 0,
                                class: "m-board-empty m-muted q-pa-md"
                              }, _toDisplayString(_ctx.labels.noCards), 1))
                            : _createCommentVNode("", true)
                        ], 2))
                      : _createCommentVNode("", true),
                    (_ctx.current === 'calendar')
                      ? (_openBlock(), _createElementBlock("div", {
                          key: 2,
                          class: _normalizeClass(["m-calendar", {'m-calendar--dragging': !!_ctx.drag && _ctx.drag.moved}])
                        }, [
                          _createElementVNode("div", { class: "m-calendar-bar row items-center no-wrap" }, [
                            _createVNode(_component_q_btn, {
                              unelevated: "",
                              dense: "",
                              "no-caps": "",
                              class: "m-btn-tonal q-px-sm",
                              label: _ctx.labels.today,
                              onClick: _ctx.goToday
                            }, null, 8, ["label", "onClick"]),
                            (_ctx.calMode !== 'list')
                              ? (_openBlock(), _createElementBlock(_Fragment, { key: 0 }, [
                                  _createVNode(_component_q_btn, {
                                    flat: "",
                                    dense: "",
                                    round: "",
                                    icon: "chevron_left",
                                    class: "m-cal-prev",
                                    onClick: $event => (_ctx.shift(-1))
                                  }, {
                                    default: _withCtx(() => [
                                      _createVNode(_component_q_tooltip, null, {
                                        default: _withCtx(() => [
                                          _createTextVNode(_toDisplayString(_ctx.labels.previous), 1)
                                        ]),
                                        _: 1
                                      })
                                    ]),
                                    _: 1
                                  }, 8, ["onClick"]),
                                  _createVNode(_component_q_btn, {
                                    flat: "",
                                    dense: "",
                                    round: "",
                                    icon: "chevron_right",
                                    class: "m-cal-next",
                                    onClick: $event => (_ctx.shift(1))
                                  }, {
                                    default: _withCtx(() => [
                                      _createVNode(_component_q_tooltip, null, {
                                        default: _withCtx(() => [
                                          _createTextVNode(_toDisplayString(_ctx.labels.next), 1)
                                        ]),
                                        _: 1
                                      })
                                    ]),
                                    _: 1
                                  }, 8, ["onClick"])
                                ], 64))
                              : _createCommentVNode("", true),
                            _createElementVNode("div", {
                              class: _normalizeClass(["m-calendar-title ellipsis q-ml-sm", _ctx.phone ? 'text-subtitle1 text-weight-medium' : 'text-h6'])
                            }, _toDisplayString(_ctx.rangeTitle), 3),
                            _createVNode(_component_q_space),
                            (_ctx.modeOptions.length > 1)
                              ? (_openBlock(), _createBlock(_component_q_btn_toggle, {
                                  key: 1,
                                  "model-value": _ctx.calMode,
                                  "onUpdate:modelValue": _ctx.setMode,
                                  dense: "",
                                  "no-caps": "",
                                  unelevated: "",
                                  "toggle-color": "primary",
                                  class: "m-cal-modes",
                                  options: _ctx.modeOptions
                                }, null, 8, ["model-value", "onUpdate:modelValue", "options"]))
                              : _createCommentVNode("", true)
                          ]),
                          (_ctx.calMode === 'month')
                            ? (_openBlock(), _createBlock(_component_q_card, {
                                key: 0,
                                flat: "",
                                bordered: "",
                                class: "m-cal-month"
                              }, {
                                default: _withCtx(() => [
                                  _createElementVNode("div", { class: "row no-wrap m-cal-dow" }, [
                                    (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.weekdayNames, (name) => {
                                      return (_openBlock(), _createElementBlock("div", {
                                        key: name,
                                        class: "col text-center text-caption m-muted"
                                      }, _toDisplayString(name), 1))
                                    }), 128))
                                  ]),
                                  (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.monthWeeks, (week) => {
                                    return (_openBlock(), _createElementBlock("div", {
                                      key: week.key,
                                      class: "row no-wrap m-cal-week"
                                    }, [
                                      (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(week.days, (cell) => {
                                        return (_openBlock(), _createElementBlock("div", {
                                          key: cell.day,
                                          class: _normalizeClass(["col m-cal-cell", {'m-cal-cell--out': !cell.inMonth, 'm-cal-cell--sel': _ctx.selection && _ctx.selection.day === cell.day}]),
                                          "data-day": cell.day,
                                          "data-zone": "month",
                                          onPointerdown: $event => (_ctx.onCellDown($event, cell.day, 'month', null))
                                        }, [
                                          _createElementVNode("div", { class: "m-cal-daynum" }, [
                                            _createElementVNode("span", {
                                              class: _normalizeClass({'m-cal-today': cell.isToday}),
                                              onPointerdown: _withModifiers(() => {}, ["stop"]),
                                              onClick: _withModifiers($event => (_ctx.goDay(cell.day)), ["stop"])
                                            }, _toDisplayString(cell.dayNum), 43, ["onPointerdown", "onClick"])
                                          ]),
                                          (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(cell.items.slice(0, cell.limit), (ev) => {
                                            return (_openBlock(), _createElementBlock("div", {
                                              key: ev.key,
                                              class: _normalizeClass(["m-cal-chip ellipsis", _ctx.chipClass(ev)]),
                                              "data-index": ev.index,
                                              onPointerdown: _withModifiers($event => (_ctx.onEventDown($event, ev, 'move')), ["stop"])
                                            }, [
                                              (ev.allDay)
                                                ? (_openBlock(), _createElementBlock(_Fragment, { key: 0 }, [
                                                    _createTextVNode(_toDisplayString(ev.title), 1)
                                                  ], 64))
                                                : (_openBlock(), _createElementBlock(_Fragment, { key: 1 }, [
                                                    _createElementVNode("span", {
                                                      class: _normalizeClass(["m-cal-dot", 'bg-' + ev.color])
                                                    }, null, 2),
                                                    _createElementVNode("span", { class: "m-cal-chip-time" }, _toDisplayString(ev.shortTime), 1),
                                                    _createTextVNode(" " + _toDisplayString(ev.title), 1)
                                                  ], 64))
                                            ], 42, ["data-index", "onPointerdown"]))
                                          }), 128)),
                                          (cell.items.length > cell.limit)
                                            ? (_openBlock(), _createElementBlock("div", {
                                                key: 0,
                                                class: "m-cal-more text-primary",
                                                onPointerdown: _withModifiers(() => {}, ["stop"]),
                                                onClick: _withModifiers($event => (_ctx.goDay(cell.day)), ["stop"])
                                              }, "+" + _toDisplayString(cell.items.length - cell.limit) + " " + _toDisplayString(_ctx.labels.more), 41, ["onPointerdown", "onClick"]))
                                            : _createCommentVNode("", true)
                                        ], 42, ["data-day", "onPointerdown"]))
                                      }), 128))
                                    ]))
                                  }), 128))
                                ]),
                                _: 1
                              }))
                            : (_ctx.calMode === 'week' || _ctx.calMode === 'day')
                              ? (_openBlock(), _createBlock(_component_q_card, {
                                  key: 1,
                                  flat: "",
                                  bordered: "",
                                  class: "m-cal-grid"
                                }, {
                                  default: _withCtx(() => [
                                    _createElementVNode("div", { class: "row no-wrap m-cal-head" }, [
                                      _createElementVNode("div", { class: "m-cal-gutter" }),
                                      (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.gridCols, (col) => {
                                        return (_openBlock(), _createElementBlock("div", {
                                          key: col.key,
                                          class: _normalizeClass(["col m-cal-head-cell text-center ellipsis", {'m-cal-head-cell--today': col.isToday}]),
                                          onClick: $event => (col.resource === null ? _ctx.goDay(col.day) : null)
                                        }, [
                                          _createElementVNode("div", {
                                            class: _normalizeClass(["text-caption", col.isToday ? 'text-primary text-weight-medium' : 'm-muted'])
                                          }, _toDisplayString(col.top), 3),
                                          _createElementVNode("div", { class: "m-cal-head-label" }, _toDisplayString(col.label), 1)
                                        ], 10, ["onClick"]))
                                      }), 128))
                                    ]),
                                    _createElementVNode("div", { class: "row no-wrap m-cal-allday" }, [
                                      _createElementVNode("div", { class: "m-cal-gutter text-caption m-muted" }, _toDisplayString(_ctx.labels.allDay), 1),
                                      (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.gridCols, (col) => {
                                        return (_openBlock(), _createElementBlock("div", {
                                          key: col.key,
                                          class: "col m-cal-allday-cell",
                                          "data-day": col.day,
                                          "data-res": col.resource,
                                          "data-zone": "allday",
                                          onPointerdown: $event => (_ctx.onCellDown($event, col.day, 'allday', col.resource))
                                        }, [
                                          (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(col.allDay, (ev) => {
                                            return (_openBlock(), _createElementBlock("div", {
                                              key: ev.key,
                                              class: _normalizeClass(["m-cal-chip ellipsis", _ctx.chipClass(ev)]),
                                              "data-index": ev.index,
                                              onPointerdown: _withModifiers($event => (_ctx.onEventDown($event, ev, 'move')), ["stop"])
                                            }, _toDisplayString(ev.title), 43, ["data-index", "onPointerdown"]))
                                          }), 128))
                                        ], 40, ["data-day", "data-res", "onPointerdown"]))
                                      }), 128))
                                    ]),
                                    _createElementVNode("div", {
                                      ref: "calScroll",
                                      class: "m-cal-scroll"
                                    }, [
                                      _createElementVNode("div", {
                                        class: "row no-wrap m-cal-hours",
                                        style: _normalizeStyle({height: (24 * _ctx.hourPx) + 'px'})
                                      }, [
                                        _createElementVNode("div", { class: "m-cal-gutter relative-position" }, [
                                          (_openBlock(), _createElementBlock(_Fragment, null, _renderList(23, (hour) => {
                                            return _createElementVNode("div", {
                                              key: hour,
                                              class: "m-cal-hour-label text-caption m-muted",
                                              style: _normalizeStyle({top: (hour * _ctx.hourPx - 8) + 'px'})
                                            }, _toDisplayString(_ctx.hourLabel(hour)), 5)
                                          }), 64))
                                        ]),
                                        (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.gridCols, (col) => {
                                          return (_openBlock(), _createElementBlock("div", {
                                            key: col.key,
                                            class: "col m-cal-col",
                                            style: _normalizeStyle({backgroundSize: '100% ' + _ctx.hourPx + 'px'}),
                                            "data-day": col.day,
                                            "data-res": col.resource,
                                            "data-zone": "time",
                                            onPointerdown: $event => (_ctx.onCellDown($event, col.day, 'time', col.resource))
                                          }, [
                                            (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(col.timed, (place) => {
                                              return (_openBlock(), _createElementBlock("div", {
                                                key: place.ev.key,
                                                class: _normalizeClass(["m-cal-event", _ctx.eventClass(place.ev)]),
                                                style: _normalizeStyle(_ctx.eventStyle(place)),
                                                "data-index": place.ev.index,
                                                onPointerdown: _withModifiers($event => (_ctx.onEventDown($event, place.ev, 'move')), ["stop"])
                                              }, [
                                                _createElementVNode("div", { class: "m-cal-event-title ellipsis" }, _toDisplayString(place.ev.title), 1),
                                                ((place.ev.end - place.ev.start) >= 45)
                                                  ? (_openBlock(), _createElementBlock("div", {
                                                      key: 0,
                                                      class: "m-cal-event-time ellipsis"
                                                    }, _toDisplayString(place.ev.timeText), 1))
                                                  : _createCommentVNode("", true),
                                                (_ctx.canMove && !_ctx.phone)
                                                  ? (_openBlock(), _createElementBlock("div", {
                                                      key: 1,
                                                      class: "m-cal-resize",
                                                      onPointerdown: _withModifiers($event => (_ctx.onEventDown($event, place.ev, 'resize')), ["stop"])
                                                    }, null, 40, ["onPointerdown"]))
                                                  : _createCommentVNode("", true)
                                              ], 46, ["data-index", "onPointerdown"]))
                                            }), 128)),
                                            (_ctx.selection && _ctx.selection.zone === 'time' && _ctx.selection.col === col.key)
                                              ? (_openBlock(), _createElementBlock("div", {
                                                  key: 0,
                                                  class: "m-cal-selection",
                                                  style: _normalizeStyle(_ctx.spanStyle(_ctx.selection.start, _ctx.selection.end, col.day))
                                                }, _toDisplayString(_ctx.selectionText), 5))
                                              : _createCommentVNode("", true),
                                            (col.isToday)
                                              ? (_openBlock(), _createElementBlock("div", {
                                                  key: 1,
                                                  class: "m-cal-now",
                                                  style: _normalizeStyle({top: (_ctx.nowMinutes / 60 * _ctx.hourPx) + 'px'})
                                                }, null, 4))
                                              : _createCommentVNode("", true)
                                          ], 44, ["data-day", "data-res", "onPointerdown"]))
                                        }), 128))
                                      ], 4)
                                    ], 512)
                                  ]),
                                  _: 1
                                }))
                              : (_openBlock(), _createBlock(_component_q_card, {
                                  key: 2,
                                  flat: "",
                                  bordered: "",
                                  class: "m-cal-agenda"
                                }, {
                                  default: _withCtx(() => [
                                    (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.agenda, (group, groupIndex) => {
                                      return (_openBlock(), _createElementBlock(_Fragment, {
                                        key: group.day
                                      }, [
                                        (groupIndex > 0)
                                          ? (_openBlock(), _createBlock(_component_q_separator, { key: 0 }))
                                          : _createCommentVNode("", true),
                                        _createElementVNode("div", {
                                          class: _normalizeClass(["m-cal-agenda-day text-subtitle2 q-px-md q-pt-sm", group.isToday ? 'text-primary' : ''])
                                        }, _toDisplayString(group.label), 3),
                                        _createVNode(_component_q_list, {
                                          dense: "",
                                          class: "q-pb-xs"
                                        }, {
                                          default: _withCtx(() => [
                                            (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(group.items, (ev) => {
                                              return (_openBlock(), _createBlock(_component_q_item, {
                                                key: ev.key,
                                                clickable: "",
                                                class: "m-cal-agenda-item",
                                                "data-index": ev.index,
                                                onClick: $event => (_ctx.openEvent($event, ev))
                                              }, {
                                                default: _withCtx(() => [
                                                  _createVNode(_component_q_item_section, {
                                                    side: "",
                                                    class: "m-cal-agenda-time text-caption"
                                                  }, {
                                                    default: _withCtx(() => [
                                                      _createTextVNode(_toDisplayString(ev.allDay ? _ctx.labels.allDay : (_ctx.phone ? ev.shortTime : ev.timeText)), 1)
                                                    ]),
                                                    _: 2
                                                  }, 1024),
                                                  _createVNode(_component_q_item_section, {
                                                    avatar: "",
                                                    class: "m-cal-agenda-dot"
                                                  }, {
                                                    default: _withCtx(() => [
                                                      _createElementVNode("span", {
                                                        class: _normalizeClass(["m-cal-dot", 'bg-' + ev.color])
                                                      }, null, 2)
                                                    ]),
                                                    _: 2
                                                  }, 1024),
                                                  _createVNode(_component_q_item_section, null, {
                                                    default: _withCtx(() => [
                                                      _createVNode(_component_q_item_label, null, {
                                                        default: _withCtx(() => [
                                                          _createTextVNode(_toDisplayString(ev.title), 1)
                                                        ]),
                                                        _: 2
                                                      }, 1024),
                                                      (ev.resourceLabel)
                                                        ? (_openBlock(), _createBlock(_component_q_item_label, {
                                                            key: 0,
                                                            caption: ""
                                                          }, {
                                                            default: _withCtx(() => [
                                                              _createTextVNode(_toDisplayString(ev.resourceLabel), 1)
                                                            ]),
                                                            _: 2
                                                          }, 1024))
                                                        : _createCommentVNode("", true)
                                                    ]),
                                                    _: 2
                                                  }, 1024)
                                                ]),
                                                _: 2
                                              }, 1032, ["data-index", "onClick"]))
                                            }), 128))
                                          ]),
                                          _: 2
                                        }, 1024)
                                      ], 64))
                                    }), 128)),
                                    (!_ctx.agenda.length)
                                      ? (_openBlock(), _createElementBlock("div", {
                                          key: 0,
                                          class: "q-pa-md m-muted"
                                        }, _toDisplayString(_ctx.labels.noEvents), 1))
                                      : _createCommentVNode("", true)
                                  ]),
                                  _: 1
                                })),
                          _createElementVNode("div", {
                            id: _ctx.uid + '_anchor',
                            class: "m-cal-anchor",
                            style: _normalizeStyle({left: _ctx.anchorRect.left + 'px', top: _ctx.anchorRect.top + 'px', width: _ctx.anchorRect.width + 'px', height: _ctx.anchorRect.height + 'px'})
                          }, null, 12, ["id"]),
                          _createVNode(_component_q_menu, {
                            modelValue: _ctx.pop.open,
                            "onUpdate:modelValue": $event => ((_ctx.pop.open) = $event),
                            target: '#' + _ctx.uid + '_anchor',
                            "no-parent-event": "",
                            anchor: "bottom left",
                            self: "top left",
                            class: "m-cal-pop"
                          }, {
                            default: _withCtx(() => [
                              (_ctx.pop.index !== null)
                                ? (_openBlock(), _createElementBlock("div", {
                                    key: 0,
                                    class: "m-cal-pop-body q-pa-sm"
                                  }, [
                                    _createElementVNode("div", { class: "text-caption m-muted q-mb-xs" }, _toDisplayString(_ctx.pop.when), 1),
                                    _renderSlot(_ctx.$slots, 'card_' + _ctx.pop.index, {
                                      index: _ctx.pop.index,
                                      row: _ctx.popRow
                                    }, () => [
                                      _renderSlot(_ctx.$slots, "card", {
                                        index: _ctx.pop.index,
                                        row: _ctx.popRow
                                      }, () => [
                                        _createElementVNode("div", { class: "m-card-title" }, _toDisplayString(_ctx.cardTitle(_ctx.popRow)), 1)
                                      ])
                                    ])
                                  ]))
                                : _createCommentVNode("", true)
                            ]),
                            _: 3
                          }, 8, ["modelValue", "onUpdate:modelValue", "target"]),
                          _createVNode(_component_m_dialog, {
                            modelValue: _ctx.create.open,
                            "onUpdate:modelValue": $event => ((_ctx.create.open) = $event),
                            title: _ctx.labels.newEvent,
                            width: "520"
                          }, {
                            default: _withCtx(() => [
                              (_ctx.create.open)
                                ? (_openBlock(), _createElementBlock(_Fragment, { key: 0 }, [
                                    (_ctx.create.url)
                                      ? (_openBlock(), _createBlock(_component_m_dynamic_container, {
                                          key: 0,
                                          id: _ctx.uid + '_create',
                                          url: _ctx.create.url
                                        }, null, 8, ["id", "url"]))
                                      : (_openBlock(), _createBlock(_component_m_form, {
                                          key: 1,
                                          action: _ctx.calendar.createTransition,
                                          "fields-initial": _ctx.create.initial,
                                          onSubmitted: _ctx.afterCreate
                                        }, {
                                          default: _withCtx((formProps) => [
                                            (_ctx.calendar.titleField)
                                              ? (_openBlock(), _createBlock(_component_m_text_line, {
                                                  key: 0,
                                                  modelValue: formProps.fields[_ctx.calendar.titleField],
                                                  "onUpdate:modelValue": $event => ((formProps.fields[_ctx.calendar.titleField]) = $event),
                                                  name: _ctx.calendar.titleField,
                                                  id: _ctx.uid + '_ctitle',
                                                  label: _ctx.labels.title,
                                                  required: ""
                                                }, null, 8, ["modelValue", "onUpdate:modelValue", "name", "id", "label"]))
                                              : _createCommentVNode("", true),
                                            _createElementVNode("div", { class: "row q-col-gutter-sm q-mt-xs" }, [
                                              _createElementVNode("div", { class: "col-12 col-sm-6" }, [
                                                _createVNode(_component_m_date_time, {
                                                  modelValue: formProps.fields[_ctx.startName],
                                                  "onUpdate:modelValue": $event => ((formProps.fields[_ctx.startName]) = $event),
                                                  name: _ctx.startName,
                                                  id: _ctx.uid + '_cstart',
                                                  type: _ctx.create.dateType,
                                                  label: _ctx.labels.starts
                                                }, null, 8, ["modelValue", "onUpdate:modelValue", "name", "id", "type", "label"])
                                              ]),
                                              (_ctx.calendar.endField)
                                                ? (_openBlock(), _createElementBlock("div", {
                                                    key: 0,
                                                    class: "col-12 col-sm-6"
                                                  }, [
                                                    _createVNode(_component_m_date_time, {
                                                      modelValue: formProps.fields[_ctx.endName],
                                                      "onUpdate:modelValue": $event => ((formProps.fields[_ctx.endName]) = $event),
                                                      name: _ctx.endName,
                                                      id: _ctx.uid + '_cend',
                                                      type: _ctx.create.dateType,
                                                      label: _ctx.labels.ends
                                                    }, null, 8, ["modelValue", "onUpdate:modelValue", "name", "id", "type", "label"])
                                                  ]))
                                                : _createCommentVNode("", true)
                                            ]),
                                            _createElementVNode("div", { class: "row justify-end q-mt-md" }, [
                                              _createVNode(_component_q_btn, {
                                                unelevated: "",
                                                dense: "",
                                                "no-caps": "",
                                                color: "primary",
                                                type: "submit",
                                                class: "q-px-md m-cal-create-submit",
                                                label: _ctx.labels.create
                                              }, null, 8, ["label"])
                                            ])
                                          ]),
                                          _: 1
                                        }, 8, ["action", "fields-initial", "onSubmitted"]))
                                  ], 64))
                                : _createCommentVNode("", true)
                            ]),
                            _: 1
                          }, 8, ["modelValue", "onUpdate:modelValue", "title"])
                        ], 2))
                      : _createCommentVNode("", true)
                  ], 10, ["onClick"]),
                  (_ctx.peekShown)
                    ? (_openBlock(), _createElementBlock("div", {
                        key: 0,
                        class: "m-peek-sep",
                        role: "separator",
                        "aria-orientation": "vertical",
                        onPointerdown: _ctx.onPeekResize
                      }, null, 40, ["onPointerdown"]))
                    : _createCommentVNode("", true),
                  (_ctx.peekShown)
                    ? (_openBlock(), _createElementBlock("div", {
                        key: 1,
                        class: "m-peek-pane",
                        style: _normalizeStyle({width: _ctx.peekWidth ? _ctx.peekWidth + 'px' : '55%'})
                      }, [
                        _createElementVNode("div", { class: "m-peek-bar row items-center no-wrap" }, [
                          _createVNode(_component_q_space),
                          (_ctx.peekKey)
                            ? (_openBlock(), _createBlock(_component_q_btn, {
                                key: 0,
                                flat: "",
                                dense: "",
                                "no-caps": "",
                                icon: "sym_o_open_in_full",
                                class: "m-peek-full",
                                label: _ctx.labels.openFull,
                                onClick: _ctx.openFull
                              }, null, 8, ["label", "onClick"]))
                            : _createCommentVNode("", true),
                          _createVNode(_component_q_btn, {
                            flat: "",
                            dense: "",
                            round: "",
                            icon: "sym_o_close",
                            class: "m-peek-close",
                            onClick: $event => (_ctx.peekSelect(''))
                          }, {
                            default: _withCtx(() => [
                              _createVNode(_component_q_tooltip, null, {
                                default: _withCtx(() => [
                                  _createTextVNode(_toDisplayString(_ctx.labels.close), 1)
                                ]),
                                _: 1
                              })
                            ]),
                            _: 1
                          }, 8, ["onClick"])
                        ]),
                        (_ctx.peekSrc)
                          ? (_openBlock(), _createBlock(_component_m_dynamic_container, {
                              key: _ctx.peekSrc,
                              id: _ctx.uid + '_peek',
                              url: _ctx.peekSrc
                            }, null, 8, ["id", "url"]))
                          : (_openBlock(), _createElementBlock("div", {
                              key: 1,
                              class: "m-peek-empty m-muted q-pa-md"
                            }, _toDisplayString(_ctx.labels.peekEmpty), 1))
                      ], 4))
                    : _createCommentVNode("", true)
                ], 2)
              ], 10, ["id", "data-view"]))
            }
        } },
        "m-markdown": { hash: "ca4e1146", make: function (Vue) {
            const { toDisplayString: _toDisplayString, openBlock: _openBlock, createElementBlock: _createElementBlock, createCommentVNode: _createCommentVNode } = Vue

            return function render(_ctx, _cache) {
              return (_openBlock(), _createElementBlock("div", {
                class: "m-markdown",
                id: _ctx.id,
                onClick: _ctx.onClick
              }, [
                (_ctx.failed)
                  ? (_openBlock(), _createElementBlock("div", {
                      key: 0,
                      class: "m-markdown-plain"
                    }, _toDisplayString(_ctx.text), 1))
                  : (_openBlock(), _createElementBlock("div", {
                      key: 1,
                      class: "m-markdown-body",
                      innerHTML: _ctx.html
                    }, null, 8, ["innerHTML"]))
              ], 8, ["id", "onClick"]))
            }
        } },
        "m-menu-icon": { hash: "1d47e7c6", make: function (Vue) {
            const { normalizeClass: _normalizeClass, openBlock: _openBlock, createElementBlock: _createElementBlock, createCommentVNode: _createCommentVNode, resolveComponent: _resolveComponent, createBlock: _createBlock } = Vue

            return function render(_ctx, _cache) {
              const _component_q_icon = _resolveComponent("q-icon")

              return (_ctx.isClass)
                ? (_openBlock(), _createElementBlock("i", {
                    key: 0,
                    class: _normalizeClass(_ctx.name)
                  }, null, 2))
                : (_openBlock(), _createBlock(_component_q_icon, {
                    key: 1,
                    name: _ctx.name,
                    size: _ctx.size
                  }, null, 8, ["name", "size"]))
            }
        } },
        "m-menu-item-content": { hash: "29c35951", make: function (Vue) {
            const { resolveComponent: _resolveComponent, openBlock: _openBlock, createBlock: _createBlock, createCommentVNode: _createCommentVNode, createElementBlock: _createElementBlock, withCtx: _withCtx, toDisplayString: _toDisplayString, createTextVNode: _createTextVNode, normalizeClass: _normalizeClass, createVNode: _createVNode, Fragment: _Fragment } = Vue

            return function render(_ctx, _cache) {
              const _component_m_menu_icon = _resolveComponent("m-menu-icon")
              const _component_q_item_section = _resolveComponent("q-item-section")

              return (_openBlock(), _createElementBlock(_Fragment, null, [
                (_ctx.menuItem.image)
                  ? (_openBlock(), _createBlock(_component_q_item_section, {
                      key: 0,
                      avatar: ""
                    }, {
                      default: _withCtx(() => [
                        (_ctx.menuItem.imageType === 'icon')
                          ? (_openBlock(), _createBlock(_component_m_menu_icon, {
                              key: 0,
                              name: _ctx.menuItem.image,
                              size: "xs"
                            }, null, 8, ["name"]))
                          : (_openBlock(), _createElementBlock("img", {
                              key: 1,
                              src: _ctx.menuItem.image,
                              alt: _ctx.menuItem.title,
                              height: "14",
                              class: "invertible"
                            }, null, 8, ["src", "alt"]))
                      ]),
                      _: 1
                    }))
                  : _createCommentVNode("", true),
                _createVNode(_component_q_item_section, {
                  class: _normalizeClass({'text-primary text-weight-medium':_ctx.active})
                }, {
                  default: _withCtx(() => [
                    _createTextVNode(_toDisplayString(_ctx.menuItem.title), 1)
                  ]),
                  _: 1
                }, 8, ["class"])
              ], 64))
            }
        } },
        "m-menu-nav-item": { hash: "a691a42d", make: function (Vue) {
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
                    "expand-icon-class": "text-grey-6",
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
                      "expand-icon-class": "text-grey-6",
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
                            "expand-icon-class": "text-grey-6",
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
                        "expand-icon-class": "text-grey-6",
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
                          "expand-icon-class": "text-grey-6",
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
        "m-menu-subscreen-item": { hash: "caf03d9", make: function (Vue) {
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
                    "expand-icon-class": "text-grey-6",
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
        "m-number-card": { hash: "6622601f", make: function (Vue) {
            const { toDisplayString: _toDisplayString, createElementVNode: _createElementVNode, normalizeClass: _normalizeClass, resolveComponent: _resolveComponent, createVNode: _createVNode, openBlock: _openBlock, createElementBlock: _createElementBlock, createCommentVNode: _createCommentVNode, createBlock: _createBlock, withCtx: _withCtx, resolveDynamicComponent: _resolveDynamicComponent } = Vue

            return function render(_ctx, _cache) {
              const _component_q_icon = _resolveComponent("q-icon")
              const _component_m_menu_icon = _resolveComponent("m-menu-icon")
              const _component_q_card_section = _resolveComponent("q-card-section")
              const _component_q_card = _resolveComponent("q-card")

              return (_openBlock(), _createBlock(_resolveDynamicComponent(_ctx.wrapper), {
                href: _ctx.href || undefined,
                class: _normalizeClass(["m-number-card-wrap", {'m-number-card-link': !!_ctx.href}])
              }, {
                default: _withCtx(() => [
                  _createVNode(_component_q_card, {
                    flat: "",
                    bordered: "",
                    class: _normalizeClass(["m-number-card", 'm-number-card-' + _ctx.size])
                  }, {
                    default: _withCtx(() => [
                      _createVNode(_component_q_card_section, { class: "row no-wrap items-start q-pa-md" }, {
                        default: _withCtx(() => [
                          _createElementVNode("div", { class: "col m-number-card-body" }, [
                            _createElementVNode("div", { class: "text-overline m-number-card-title ellipsis" }, _toDisplayString(_ctx.title), 1),
                            _createElementVNode("div", {
                              class: _normalizeClass(["m-number-card-value", _ctx.valueClass])
                            }, _toDisplayString(_ctx.shownValue), 3),
                            (_ctx.deltaInfo)
                              ? (_openBlock(), _createElementBlock("div", {
                                  key: 0,
                                  class: _normalizeClass(["text-caption row no-wrap items-center m-number-card-delta", _ctx.deltaClass])
                                }, [
                                  _createVNode(_component_q_icon, {
                                    name: _ctx.deltaInfo.icon,
                                    size: "xs",
                                    class: "q-mr-xs"
                                  }, null, 8, ["name"]),
                                  _createElementVNode("span", null, _toDisplayString(_ctx.delta), 1)
                                ], 2))
                              : _createCommentVNode("", true)
                          ]),
                          (_ctx.icon)
                            ? (_openBlock(), _createBlock(_component_m_menu_icon, {
                                key: 0,
                                name: _ctx.icon,
                                size: "sm",
                                class: "m-number-card-icon"
                              }, null, 8, ["name"]))
                            : _createCommentVNode("", true)
                        ]),
                        _: 1
                      })
                    ]),
                    _: 1
                  }, 8, ["class"])
                ]),
                _: 1
              }, 8, ["href", "class"]))
            }
        } },
        "m-qapps-root": { hash: "97f8b1e2", make: function (Vue) {
            const { resolveComponent: _resolveComponent, createVNode: _createVNode, openBlock: _openBlock, createElementBlock: _createElementBlock, createCommentVNode: _createCommentVNode, createBlock: _createBlock, toDisplayString: _toDisplayString, createTextVNode: _createTextVNode, withCtx: _withCtx, renderList: _renderList, Fragment: _Fragment, createElementVNode: _createElementVNode, normalizeClass: _normalizeClass, resolveDirective: _resolveDirective, withDirectives: _withDirectives, resolveDynamicComponent: _resolveDynamicComponent, vShow: _vShow, withModifiers: _withModifiers } = Vue

            return function render(_ctx, _cache) {
              const _component_q_btn = _resolveComponent("q-btn")
              const _component_q_icon = _resolveComponent("q-icon")
              const _component_q_tooltip = _resolveComponent("q-tooltip")
              const _component_m_menu_icon = _resolveComponent("m-menu-icon")
              const _component_q_separator = _resolveComponent("q-separator")
              const _component_q_menu = _resolveComponent("q-menu")
              const _component_q_item_section = _resolveComponent("q-item-section")
              const _component_q_item = _resolveComponent("q-item")
              const _component_q_list = _resolveComponent("q-list")
              const _component_q_toolbar_title = _resolveComponent("q-toolbar-title")
              const _component_q_breadcrumbs_el = _resolveComponent("q-breadcrumbs-el")
              const _component_q_breadcrumbs = _resolveComponent("q-breadcrumbs")
              const _component_q_space = _resolveComponent("q-space")
              const _component_q_badge = _resolveComponent("q-badge")
              const _component_m_dynamic_dialog = _resolveComponent("m-dynamic-dialog")
              const _component_q_item_label = _resolveComponent("q-item-label")
              const _component_m_avatar = _resolveComponent("m-avatar")
              const _component_m_user_status_menu = _resolveComponent("m-user-status-menu")
              const _component_q_toggle = _resolveComponent("q-toggle")
              const _component_m_assist_button = _resolveComponent("m-assist-button")
              const _component_q_toolbar = _resolveComponent("q-toolbar")
              const _component_q_header = _resolveComponent("q-header")
              const _component_m_menu_nav_item = _resolveComponent("m-menu-nav-item")
              const _component_q_scroll_area = _resolveComponent("q-scroll-area")
              const _component_q_drawer = _resolveComponent("q-drawer")
              const _component_m_shell_assist = _resolveComponent("m-shell-assist")
              const _component_m_subscreens_active = _resolveComponent("m-subscreens-active")
              const _component_q_page = _resolveComponent("q-page")
              const _component_m_assist_canvas = _resolveComponent("m-assist-canvas")
              const _component_m_shell_tray = _resolveComponent("m-shell-tray")
              const _component_q_page_container = _resolveComponent("q-page-container")
              const _component_q_footer = _resolveComponent("q-footer")
              const _component_q_layout = _resolveComponent("q-layout")
              const _component_m_text_line = _resolveComponent("m-text-line")
              const _component_q_form = _resolveComponent("q-form")
              const _component_m_dialog = _resolveComponent("m-dialog")
              const _component_m_shell_palette = _resolveComponent("m-shell-palette")
              const _component_m_shell_quick_create = _resolveComponent("m-shell-quick-create")
              const _component_m_shell_keys = _resolveComponent("m-shell-keys")
              const _component_m_user_status_dialog = _resolveComponent("m-user-status-dialog")
              const _directive_close_popup = _resolveDirective("close-popup")

              return (_openBlock(), _createElementBlock(_Fragment, null, [
                _createVNode(_component_q_layout, { view: "hHh LpR fFf" }, {
                  default: _withCtx(() => [
                    _createVNode(_component_q_header, {
                      bordered: "",
                      id: "top",
                      class: _normalizeClass(_ctx.$q.dark.isActive ? 'bg-dark text-white' : 'bg-white text-dark')
                    }, {
                      default: _withCtx(() => [
                        _createVNode(_component_q_toolbar, { dense: "" }, {
                          default: _withCtx(() => [
                            _createVNode(_component_q_btn, {
                              flat: "",
                              round: "",
                              dense: "",
                              icon: "menu",
                              onClick: $event => (_ctx.toggleLeftOpen())
                            }, null, 8, ["onClick"]),
                            _createVNode(_component_q_btn, {
                              flat: "",
                              dense: "",
                              "no-caps": "",
                              id: "m-launcher",
                              class: "q-mx-xs",
                              "aria-label": _ctx.labels.applications
                            }, {
                              default: _withCtx(() => [
                                (_ctx.shell.logoUrl)
                                  ? (_openBlock(), _createElementBlock("img", {
                                      key: 0,
                                      src: _ctx.shell.logoUrl,
                                      alt: "",
                                      height: "28"
                                    }, null, 8, ["src"]))
                                  : (_openBlock(), _createBlock(_component_q_icon, {
                                      key: 1,
                                      name: "sym_o_apps"
                                    })),
                                _createVNode(_component_q_tooltip, null, {
                                  default: _withCtx(() => [
                                    _createTextVNode(_toDisplayString(_ctx.labels.applications), 1)
                                  ]),
                                  _: 1
                                }),
                                _createVNode(_component_q_menu, {
                                  anchor: "bottom left",
                                  self: "top left",
                                  class: "m-launcher"
                                }, {
                                  default: _withCtx(() => [
                                    _createElementVNode("div", { class: "q-pa-sm" }, [
                                      _createElementVNode("div", { class: "m-launcher-grid" }, [
                                        (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.shellApps, (app) => {
                                          return _withDirectives((_openBlock(), _createBlock(_component_q_btn, {
                                            key: app.name,
                                            unelevated: "",
                                            "no-caps": "",
                                            class: _normalizeClass(["m-btn-tonal m-launcher-app", {'m-launcher-current': _ctx.currentApp && app.name === _ctx.currentApp.name}]),
                                            onClick: $event => (_ctx.setUrl(app.path))
                                          }, {
                                            default: _withCtx(() => [
                                              _createElementVNode("div", { class: "column items-center no-wrap full-width" }, [
                                                (app.image && app.imageType === 'icon')
                                                  ? (_openBlock(), _createBlock(_component_m_menu_icon, {
                                                      key: 0,
                                                      name: app.image,
                                                      size: "md"
                                                    }, null, 8, ["name"]))
                                                  : (app.image)
                                                    ? (_openBlock(), _createElementBlock("img", {
                                                        key: 1,
                                                        src: app.image,
                                                        alt: app.title,
                                                        height: "28",
                                                        class: "invertible"
                                                      }, null, 8, ["src", "alt"]))
                                                    : (_openBlock(), _createBlock(_component_q_icon, {
                                                        key: 2,
                                                        name: "sym_o_apps",
                                                        size: "md"
                                                      })),
                                                _createElementVNode("div", { class: "m-launcher-title" }, _toDisplayString(app.title), 1)
                                              ])
                                            ]),
                                            _: 2
                                          }, 1032, ["class", "onClick"])), [
                                            [_directive_close_popup]
                                          ])
                                        }), 128))
                                      ]),
                                      _createVNode(_component_q_separator, { class: "q-my-sm" }),
                                      _withDirectives(_createVNode(_component_q_btn, {
                                        flat: "",
                                        "no-caps": "",
                                        dense: "",
                                        icon: "sym_o_apps",
                                        label: _ctx.labels.allApplications,
                                        onClick: $event => (_ctx.setUrl(_ctx.linkBasePath + '/AppList'))
                                      }, null, 8, ["label", "onClick"]), [
                                        [_directive_close_popup]
                                      ])
                                    ])
                                  ]),
                                  _: 1
                                })
                              ]),
                              _: 1
                            }, 8, ["aria-label"]),
                            (_ctx.appScreenMenu)
                              ? (_openBlock(), _createBlock(_component_q_btn, {
                                  key: 0,
                                  flat: "",
                                  dense: "",
                                  "no-caps": "",
                                  id: "m-app-screens",
                                  class: "gt-xs m-app-screens text-subtitle1 text-weight-medium"
                                }, {
                                  default: _withCtx(() => [
                                    _createTextVNode(_toDisplayString(_ctx.headerTitle), 1),
                                    _createVNode(_component_q_icon, {
                                      name: "arrow_drop_down",
                                      size: "xs"
                                    }),
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
                                            (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.appScreenMenu.subscreens, (subscreen) => {
                                              return _withDirectives((_openBlock(), _createBlock(_component_q_item, {
                                                key: subscreen.name,
                                                clickable: "",
                                                active: subscreen.active,
                                                "active-class": "text-primary text-weight-medium",
                                                to: subscreen.pathWithParams
                                              }, {
                                                default: _withCtx(() => [
                                                  _createVNode(_component_q_item_section, { avatar: "" }, {
                                                    default: _withCtx(() => [
                                                      (subscreen.image && subscreen.imageType === 'icon')
                                                        ? (_openBlock(), _createBlock(_component_m_menu_icon, {
                                                            key: 0,
                                                            name: subscreen.image,
                                                            size: "xs"
                                                          }, null, 8, ["name"]))
                                                        : (subscreen.image)
                                                          ? (_openBlock(), _createElementBlock("img", {
                                                              key: 1,
                                                              src: subscreen.image,
                                                              alt: subscreen.title,
                                                              width: "18",
                                                              class: "invertible"
                                                            }, null, 8, ["src", "alt"]))
                                                          : (_openBlock(), _createBlock(_component_q_icon, {
                                                              key: 2,
                                                              name: "chevron_right",
                                                              size: "xs"
                                                            }))
                                                    ]),
                                                    _: 2
                                                  }, 1024),
                                                  _createVNode(_component_q_item_section, null, {
                                                    default: _withCtx(() => [
                                                      _createTextVNode(_toDisplayString(subscreen.title), 1)
                                                    ]),
                                                    _: 2
                                                  }, 1024)
                                                ]),
                                                _: 2
                                              }, 1032, ["active", "to"])), [
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
                                }))
                              : (_ctx.headerTitle)
                                ? (_openBlock(), _createBlock(_component_q_toolbar_title, {
                                    key: 1,
                                    shrink: "",
                                    class: "gt-xs text-subtitle1 text-weight-medium q-pr-sm"
                                  }, {
                                    default: _withCtx(() => [
                                      _createTextVNode(_toDisplayString(_ctx.headerTitle), 1)
                                    ]),
                                    _: 1
                                  }))
                                : _createCommentVNode("", true),
                            _createVNode(_component_q_breadcrumbs, {
                              id: "m-breadcrumbs",
                              class: "gt-xs",
                              gutter: "xs",
                              "separator-color": "grey-5",
                              "active-color": "grey-8"
                            }, {
                              default: _withCtx(() => [
                                (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.navMenuList, (navMenuItem, menuIndex) => {
                                  return (_openBlock(), _createElementBlock(_Fragment, {
                                    key: navMenuItem.path || menuIndex
                                  }, [
                                    (menuIndex >= _ctx.breadcrumbStart)
                                      ? (_openBlock(), _createElementBlock(_Fragment, { key: 0 }, [
                                          (menuIndex === _ctx.navMenuList.length - 1)
                                            ? (_openBlock(), _createBlock(_component_q_breadcrumbs_el, {
                                                key: 0,
                                                label: navMenuItem.title,
                                                class: "text-weight-medium"
                                              }, null, 8, ["label"]))
                                            : (navMenuItem.subscreens && navMenuItem.subscreens.length && !navMenuItem.hasTabMenu)
                                              ? (_openBlock(), _createBlock(_component_q_breadcrumbs_el, {
                                                  key: 1,
                                                  class: "cursor-pointer"
                                                }, {
                                                  default: _withCtx(() => [
                                                    _createTextVNode(_toDisplayString(navMenuItem.title), 1),
                                                    _createVNode(_component_q_icon, {
                                                      name: "arrow_drop_down",
                                                      size: "xs"
                                                    }),
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
                                                                clickable: "",
                                                                active: subscreen.active,
                                                                "active-class": "text-primary text-weight-medium",
                                                                to: subscreen.pathWithParams
                                                              }, {
                                                                default: _withCtx(() => [
                                                                  _createVNode(_component_q_item_section, { avatar: "" }, {
                                                                    default: _withCtx(() => [
                                                                      (subscreen.image && subscreen.imageType === 'icon')
                                                                        ? (_openBlock(), _createBlock(_component_m_menu_icon, {
                                                                            key: 0,
                                                                            name: subscreen.image,
                                                                            size: "xs"
                                                                          }, null, 8, ["name"]))
                                                                        : (subscreen.image)
                                                                          ? (_openBlock(), _createElementBlock("img", {
                                                                              key: 1,
                                                                              src: subscreen.image,
                                                                              alt: subscreen.title,
                                                                              width: "18",
                                                                              class: "invertible"
                                                                            }, null, 8, ["src", "alt"]))
                                                                          : (_openBlock(), _createBlock(_component_q_icon, {
                                                                              key: 2,
                                                                              name: "chevron_right",
                                                                              size: "xs"
                                                                            }))
                                                                    ]),
                                                                    _: 2
                                                                  }, 1024),
                                                                  _createVNode(_component_q_item_section, null, {
                                                                    default: _withCtx(() => [
                                                                      _createTextVNode(_toDisplayString(subscreen.title), 1)
                                                                    ]),
                                                                    _: 2
                                                                  }, 1024)
                                                                ]),
                                                                _: 2
                                                              }, 1032, ["active", "to"])), [
                                                                [_directive_close_popup]
                                                              ])
                                                            }), 128))
                                                          ]),
                                                          _: 2
                                                        }, 1024)
                                                      ]),
                                                      _: 2
                                                    }, 1024)
                                                  ]),
                                                  _: 2
                                                }, 1024))
                                              : (_openBlock(), _createBlock(_component_q_breadcrumbs_el, {
                                                  key: 2,
                                                  label: navMenuItem.title,
                                                  to: _ctx.getNavHref(menuIndex)
                                                }, null, 8, ["label", "to"]))
                                        ], 64))
                                      : _createCommentVNode("", true)
                                  ], 64))
                                }), 128))
                              ]),
                              _: 1
                            }),
                            _createVNode(_component_q_space),
                            _createVNode(_component_q_btn, {
                              flat: "",
                              dense: "",
                              "no-caps": "",
                              id: "m-search",
                              icon: "sym_o_search",
                              round: !_ctx.$q.screen.gt.sm,
                              label: _ctx.$q.screen.gt.sm ? _ctx.labels.search : undefined,
                              onClick: $event => (_ctx.openPalette())
                            }, {
                              default: _withCtx(() => [
                                (_ctx.$q.screen.gt.sm)
                                  ? (_openBlock(), _createElementBlock("span", {
                                      key: 0,
                                      class: "m-kbd q-ml-sm"
                                    }, _toDisplayString(_ctx.paletteKeyHint), 1))
                                  : _createCommentVNode("", true),
                                _createVNode(_component_q_tooltip, null, {
                                  default: _withCtx(() => [
                                    _createTextVNode(_toDisplayString(_ctx.labels.search) + " (" + _toDisplayString(_ctx.paletteKeyHint) + ")", 1)
                                  ]),
                                  _: 1
                                })
                              ]),
                              _: 1
                            }, 8, ["round", "label", "onClick"]),
                            (_ctx.quickCreateItems.length)
                              ? (_openBlock(), _createBlock(_component_q_btn, {
                                  key: 2,
                                  flat: "",
                                  round: "",
                                  dense: "",
                                  id: "m-quick-create-menu",
                                  icon: "sym_o_add"
                                }, {
                                  default: _withCtx(() => [
                                    _createVNode(_component_q_tooltip, null, {
                                      default: _withCtx(() => [
                                        _createTextVNode(_toDisplayString(_ctx.labels.quickCreate), 1)
                                      ]),
                                      _: 1
                                    }),
                                    _createVNode(_component_q_menu, {
                                      anchor: "bottom right",
                                      self: "top right"
                                    }, {
                                      default: _withCtx(() => [
                                        _createVNode(_component_q_list, {
                                          dense: "",
                                          style: {"min-width":"220px"}
                                        }, {
                                          default: _withCtx(() => [
                                            (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.quickCreateItems, (item) => {
                                              return _withDirectives((_openBlock(), _createBlock(_component_q_item, {
                                                key: item.key,
                                                clickable: "",
                                                onClick: $event => (_ctx.openQuickCreate(item))
                                              }, {
                                                default: _withCtx(() => [
                                                  _createVNode(_component_q_item_section, { avatar: "" }, {
                                                    default: _withCtx(() => [
                                                      _createVNode(_component_m_menu_icon, {
                                                        name: item.icon || 'sym_o_add',
                                                        size: "xs"
                                                      }, null, 8, ["name"])
                                                    ]),
                                                    _: 2
                                                  }, 1024),
                                                  _createVNode(_component_q_item_section, null, {
                                                    default: _withCtx(() => [
                                                      _createTextVNode(_toDisplayString(item.label), 1)
                                                    ]),
                                                    _: 2
                                                  }, 1024)
                                                ]),
                                                _: 2
                                              }, 1032, ["onClick"])), [
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
                                }))
                              : _createCommentVNode("", true),
                            (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.headerCounters, (counter) => {
                              return (_openBlock(), _createBlock(_component_q_btn, {
                                key: counter.key,
                                flat: "",
                                round: "",
                                dense: "",
                                id: 'm-counter-' + counter.key,
                                icon: counter.icon,
                                class: _normalizeClass({'text-primary': counter.tray && _ctx.trayOpen && (counter.any || _ctx.trayTab === counter.key)}),
                                onClick: $event => (counter.tray ? _ctx.toggleTray(counter.any ? null : counter.key) : _ctx.setUrl(counter.url))
                              }, {
                                default: _withCtx(() => [
                                  (_ctx.counterCount(counter))
                                    ? (_openBlock(), _createBlock(_component_q_badge, {
                                        key: 0,
                                        floating: "",
                                        rounded: "",
                                        color: "negative"
                                      }, {
                                        default: _withCtx(() => [
                                          _createTextVNode(_toDisplayString(_ctx.counterText(counter)), 1)
                                        ]),
                                        _: 2
                                      }, 1024))
                                    : _createCommentVNode("", true),
                                  (counter.key === 'chat' && _ctx.myPresence)
                                    ? (_openBlock(), _createElementBlock("span", {
                                        key: 1,
                                        class: _normalizeClass(["m-avatar-presence m-shell-presence", 'm-presence-' + _ctx.myPresence])
                                      }, null, 2))
                                    : _createCommentVNode("", true),
                                  _createVNode(_component_q_tooltip, null, {
                                    default: _withCtx(() => [
                                      _createTextVNode(_toDisplayString(counter.label), 1)
                                    ]),
                                    _: 2
                                  }, 1024)
                                ]),
                                _: 2
                              }, 1032, ["id", "icon", "class", "onClick"]))
                            }), 128)),
                            (_openBlock(), _createBlock(_resolveDynamicComponent(_ctx.qzVue), { ref: "qzVue" }, null, 512)),
                            (_ctx.documentMenuList.length)
                              ? (_openBlock(), _createBlock(_component_q_btn, {
                                  key: 3,
                                  flat: "",
                                  round: "",
                                  dense: "",
                                  icon: "help_outline"
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
                                          style: {"min-width":"200px"}
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
                                }))
                              : _createCommentVNode("", true),
                            (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.navPlugins, (navPlugin) => {
                              return (_openBlock(), _createBlock(_resolveDynamicComponent(navPlugin)))
                            }), 256)),
                            (!_ctx.shellValues.trayNotifications)
                              ? (_openBlock(), _createBlock(_component_q_btn, {
                                  key: 4,
                                  flat: "",
                                  round: "",
                                  dense: "",
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
                                          separator: "",
                                          style: {"min-width":"320px","max-width":"440px"}
                                        }, {
                                          default: _withCtx(() => [
                                            (!_ctx.notifyHistoryList.length)
                                              ? (_openBlock(), _createBlock(_component_q_item, { key: 0 }, {
                                                  default: _withCtx(() => [
                                                    _createVNode(_component_q_item_section, { class: "text-grey-6" }, {
                                                      default: _withCtx(() => [
                                                        _createTextVNode(_toDisplayString(_ctx.labels.notifyHistory), 1)
                                                      ]),
                                                      _: 1
                                                    })
                                                  ]),
                                                  _: 1
                                                }))
                                              : _createCommentVNode("", true),
                                            (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.notifyHistoryList, (histItem, histIndex) => {
                                              return (_openBlock(), _createBlock(_component_q_item, {
                                                key: histIndex,
                                                clickable: !!histItem.link,
                                                onClick: $event => (histItem.link && _ctx.moqui.openLink(histItem.link))
                                              }, {
                                                default: _withCtx(() => [
                                                  _createVNode(_component_q_item_section, { avatar: "" }, {
                                                    default: _withCtx(() => [
                                                      _createVNode(_component_q_icon, {
                                                        name: _ctx.notifyIcon(histItem.type),
                                                        color: _ctx.getQuasarColor(histItem.type) || 'info'
                                                      }, null, 8, ["name", "color"])
                                                    ]),
                                                    _: 2
                                                  }, 1024),
                                                  _createVNode(_component_q_item_section, null, {
                                                    default: _withCtx(() => [
                                                      _createVNode(_component_q_item_label, null, {
                                                        default: _withCtx(() => [
                                                          _createTextVNode(_toDisplayString(histItem.message), 1)
                                                        ]),
                                                        _: 2
                                                      }, 1024),
                                                      _createVNode(_component_q_item_label, { caption: "" }, {
                                                        default: _withCtx(() => [
                                                          _createTextVNode(_toDisplayString(histItem.time), 1)
                                                        ]),
                                                        _: 2
                                                      }, 1024)
                                                    ]),
                                                    _: 2
                                                  }, 1024)
                                                ]),
                                                _: 2
                                              }, 1032, ["clickable", "onClick"]))
                                            }), 128))
                                          ]),
                                          _: 1
                                        })
                                      ]),
                                      _: 1
                                    })
                                  ]),
                                  _: 1
                                }))
                              : _createCommentVNode("", true),
                            _createVNode(_component_q_btn, {
                              flat: "",
                              round: "",
                              dense: "",
                              id: "m-account"
                            }, {
                              default: _withCtx(() => [
                                _createVNode(_component_m_avatar, {
                                  name: _ctx.userFullName || _ctx.username,
                                  "user-id": _ctx.userId,
                                  size: "28px"
                                }, null, 8, ["name", "user-id"]),
                                _createVNode(_component_q_tooltip, null, {
                                  default: _withCtx(() => [
                                    _createTextVNode(_toDisplayString(_ctx.userFullName || _ctx.labels.account), 1)
                                  ]),
                                  _: 1
                                }),
                                _createVNode(_component_q_menu, null, {
                                  default: _withCtx(() => [
                                    _createVNode(_component_q_list, { style: {"min-width":"240px"} }, {
                                      default: _withCtx(() => [
                                        (_ctx.userFullName || _ctx.username)
                                          ? (_openBlock(), _createBlock(_component_q_item, { key: 0 }, {
                                              default: _withCtx(() => [
                                                _createVNode(_component_q_item_section, null, {
                                                  default: _withCtx(() => [
                                                    _createVNode(_component_q_item_label, { class: "text-weight-medium" }, {
                                                      default: _withCtx(() => [
                                                        _createTextVNode(_toDisplayString(_ctx.userFullName || _ctx.username), 1)
                                                      ]),
                                                      _: 1
                                                    }),
                                                    (_ctx.userFullName)
                                                      ? (_openBlock(), _createBlock(_component_q_item_label, {
                                                          key: 0,
                                                          caption: ""
                                                        }, {
                                                          default: _withCtx(() => [
                                                            _createTextVNode(_toDisplayString(_ctx.username), 1)
                                                          ]),
                                                          _: 1
                                                        }))
                                                      : _createCommentVNode("", true)
                                                  ]),
                                                  _: 1
                                                })
                                              ]),
                                              _: 1
                                            }))
                                          : _createCommentVNode("", true),
                                        (_ctx.userFullName || _ctx.username)
                                          ? (_openBlock(), _createBlock(_component_q_separator, { key: 1 }))
                                          : _createCommentVNode("", true),
                                        (_ctx.chatStatus)
                                          ? (_openBlock(), _createBlock(_component_m_user_status_menu, { key: 2 }))
                                          : _createCommentVNode("", true),
                                        (_ctx.chatStatus)
                                          ? (_openBlock(), _createBlock(_component_q_separator, { key: 3 }))
                                          : _createCommentVNode("", true),
                                        (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.accountMenuItems, (item) => {
                                          return _withDirectives((_openBlock(), _createBlock(_component_q_item, {
                                            key: item.url,
                                            clickable: "",
                                            onClick: $event => (_ctx.setUrl(item.url))
                                          }, {
                                            default: _withCtx(() => [
                                              _createVNode(_component_q_item_section, { avatar: "" }, {
                                                default: _withCtx(() => [
                                                  _createVNode(_component_m_menu_icon, {
                                                    name: item.icon || 'sym_o_chevron_right'
                                                  }, null, 8, ["name"])
                                                ]),
                                                _: 2
                                              }, 1024),
                                              _createVNode(_component_q_item_section, null, {
                                                default: _withCtx(() => [
                                                  _createTextVNode(_toDisplayString(item.label), 1)
                                                ]),
                                                _: 2
                                              }, 1024)
                                            ]),
                                            _: 2
                                          }, 1032, ["onClick"])), [
                                            [_directive_close_popup]
                                          ])
                                        }), 128)),
                                        (_ctx.accountMenuItems.length)
                                          ? (_openBlock(), _createBlock(_component_q_separator, { key: 4 }))
                                          : _createCommentVNode("", true),
                                        (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.accountPlugins, (accountPlugin, accountIndex) => {
                                          return (_openBlock(), _createBlock(_component_q_item, { key: accountIndex }, {
                                            default: _withCtx(() => [
                                              _createVNode(_component_q_item_section, null, {
                                                default: _withCtx(() => [
                                                  (_openBlock(), _createBlock(_resolveDynamicComponent(accountPlugin)))
                                                ]),
                                                _: 2
                                              }, 1024)
                                            ]),
                                            _: 2
                                          }, 1024))
                                        }), 128)),
                                        (_ctx.terminalUrl)
                                          ? _withDirectives((_openBlock(), _createBlock(_component_q_item, {
                                              key: 5,
                                              id: "m-account-terminal",
                                              clickable: "",
                                              onClick: $event => (_ctx.setUrl(_ctx.terminalUrl))
                                            }, {
                                              default: _withCtx(() => [
                                                _createVNode(_component_q_item_section, { avatar: "" }, {
                                                  default: _withCtx(() => [
                                                    _createVNode(_component_q_icon, { name: "terminal" })
                                                  ]),
                                                  _: 1
                                                }),
                                                _createVNode(_component_q_item_section, null, {
                                                  default: _withCtx(() => [
                                                    _createTextVNode(_toDisplayString(_ctx.labels.terminal), 1)
                                                  ]),
                                                  _: 1
                                                })
                                              ]),
                                              _: 1
                                            }, 8, ["onClick"])), [
                                              [_directive_close_popup]
                                            ])
                                          : _createCommentVNode("", true),
                                        _createVNode(_component_q_item, {
                                          tag: "label",
                                          clickable: ""
                                        }, {
                                          default: _withCtx(() => [
                                            _createVNode(_component_q_item_section, { avatar: "" }, {
                                              default: _withCtx(() => [
                                                _createVNode(_component_q_icon, { name: "dark_mode" })
                                              ]),
                                              _: 1
                                            }),
                                            _createVNode(_component_q_item_section, null, {
                                              default: _withCtx(() => [
                                                _createTextVNode(_toDisplayString(_ctx.labels.switchDarkLight), 1)
                                              ]),
                                              _: 1
                                            }),
                                            _createVNode(_component_q_item_section, { side: "" }, {
                                              default: _withCtx(() => [
                                                _createVNode(_component_q_toggle, {
                                                  dense: "",
                                                  "model-value": _ctx.$q.dark.isActive,
                                                  "onUpdate:modelValue": $event => (_ctx.switchDarkLight())
                                                }, null, 8, ["model-value", "onUpdate:modelValue"])
                                              ]),
                                              _: 1
                                            })
                                          ]),
                                          _: 1
                                        }),
                                        _withDirectives((_openBlock(), _createBlock(_component_q_item, {
                                          clickable: "",
                                          onClick: _ctx.reLoginShowDialog
                                        }, {
                                          default: _withCtx(() => [
                                            _createVNode(_component_q_item_section, { avatar: "" }, {
                                              default: _withCtx(() => [
                                                _createVNode(_component_q_icon, { name: "autorenew" })
                                              ]),
                                              _: 1
                                            }),
                                            _createVNode(_component_q_item_section, null, {
                                              default: _withCtx(() => [
                                                _createTextVNode(_toDisplayString(_ctx.labels.reLogin), 1)
                                              ]),
                                              _: 1
                                            })
                                          ]),
                                          _: 1
                                        }, 8, ["onClick"])), [
                                          [_directive_close_popup]
                                        ]),
                                        _createVNode(_component_q_separator),
                                        _createVNode(_component_q_item, {
                                          clickable: "",
                                          tag: "a",
                                          href: _ctx.shell.logoutUrl,
                                          "data-confirm": _ctx.labels.logout + ' ' + _ctx.userFullName + '?'
                                        }, {
                                          default: _withCtx(() => [
                                            _createVNode(_component_q_item_section, { avatar: "" }, {
                                              default: _withCtx(() => [
                                                _createVNode(_component_q_icon, {
                                                  name: "logout",
                                                  color: "negative"
                                                })
                                              ]),
                                              _: 1
                                            }),
                                            _createVNode(_component_q_item_section, { class: "text-negative" }, {
                                              default: _withCtx(() => [
                                                _createTextVNode(_toDisplayString(_ctx.labels.logout), 1)
                                              ]),
                                              _: 1
                                            })
                                          ]),
                                          _: 1
                                        }, 8, ["href", "data-confirm"])
                                      ]),
                                      _: 1
                                    })
                                  ]),
                                  _: 1
                                })
                              ]),
                              _: 1
                            }),
                            (_ctx.assistOn)
                              ? (_openBlock(), _createBlock(_component_m_assist_button, { key: 5 }))
                              : _createCommentVNode("", true)
                          ]),
                          _: 1
                        })
                      ]),
                      _: 1
                    }, 8, ["class"]),
                    _createVNode(_component_q_drawer, {
                      modelValue: _ctx.leftOpen,
                      "onUpdate:modelValue": $event => ((_ctx.leftOpen) = $event),
                      side: "left",
                      bordered: "",
                      width: 260,
                      breakpoint: 1023
                    }, {
                      default: _withCtx(() => [
                        _createVNode(_component_q_scroll_area, { class: "fit" }, {
                          default: _withCtx(() => [
                            _createVNode(_component_q_list, {
                              dense: "",
                              padding: ""
                            }, {
                              default: _withCtx(() => [
                                _createVNode(_component_m_menu_nav_item, { "menu-index": 0 })
                              ]),
                              _: 1
                            })
                          ]),
                          _: 1
                        })
                      ]),
                      _: 1
                    }, 8, ["modelValue", "onUpdate:modelValue"]),
                    (_ctx.assistOn)
                      ? (_openBlock(), _createBlock(_component_m_shell_assist, { key: 0 }))
                      : _createCommentVNode("", true),
                    _createVNode(_component_q_page_container, { id: "m-canvas" }, {
                      default: _withCtx(() => [
                        _withDirectives(_createVNode(_component_q_page, { class: "q-pa-sm" }, {
                          default: _withCtx(() => [
                            _createVNode(_component_m_subscreens_active)
                          ]),
                          _: 1
                        }, 512), [
                          [_vShow, !_ctx.assistCanvasShown]
                        ]),
                        (_ctx.assistOn)
                          ? (_openBlock(), _createBlock(_component_m_assist_canvas, { key: 0 }))
                          : _createCommentVNode("", true),
                        (_ctx.trayTabs.length)
                          ? (_openBlock(), _createBlock(_component_m_shell_tray, { key: 1 }))
                          : _createCommentVNode("", true)
                      ]),
                      _: 1
                    }),
                    _createVNode(_component_q_footer, {
                      bordered: "",
                      id: "footer",
                      class: _normalizeClass(["text-caption q-pa-xs", _ctx.$q.dark.isActive ? 'bg-dark text-grey-5' : 'bg-grey-2 text-grey-8'])
                    }, {
                      default: _withCtx(() => [
                        _createElementVNode("div", {
                          innerHTML: _ctx.shell.footerHtml,
                          class: "row full-width"
                        }, null, 8, ["innerHTML"])
                      ]),
                      _: 1
                    }, 8, ["class"])
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
                          _createElementVNode("div", { class: "text-center q-pb-sm" }, [
                            _createTextVNode("User "),
                            _createElementVNode("strong", null, _toDisplayString(_ctx.username), 1),
                            _createTextVNode(" requires an authentication code, you have these options:")
                          ]),
                          _createElementVNode("div", { class: "text-center q-pb-sm" }, _toDisplayString(_ctx.reLoginMfaData.factorTypeDescriptions.join(", ")), 1),
                          _createVNode(_component_q_form, {
                            onSubmit: _withModifiers(_ctx.reLoginVerifyOtp, ["prevent"]),
                            autocapitalize: "off",
                            autocomplete: "off",
                            class: "q-gutter-y-sm"
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
                                unelevated: "",
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
                              class: "q-pt-sm"
                            }, [
                              _createVNode(_component_q_btn, {
                                unelevated: "",
                                "no-caps": "",
                                dense: "",
                                class: "m-btn-tonal",
                                label: _ctx.labels.sendCodeTo + ' ' + sendableFactor.factorOption,
                                onClick: _withModifiers($event => (_ctx.reLoginSendOtp(sendableFactor.factorId)), ["prevent"])
                              }, null, 8, ["label", "onClick"])
                            ]))
                          }), 128))
                        ]))
                      : (_openBlock(), _createElementBlock("div", { key: 1 }, [
                          _createElementVNode("div", { class: "text-center q-pb-sm" }, [
                            _createTextVNode("Please sign in to continue as user "),
                            _createElementVNode("strong", null, _toDisplayString(_ctx.username), 1)
                          ]),
                          _createVNode(_component_q_form, {
                            onSubmit: _withModifiers(_ctx.reLoginSubmit, ["prevent"]),
                            autocapitalize: "off",
                            autocomplete: "off",
                            class: "q-gutter-y-sm"
                          }, {
                            default: _withCtx(() => [
                              _createVNode(_component_m_text_line, {
                                modelValue: _ctx.reLoginPassword,
                                "onUpdate:modelValue": $event => ((_ctx.reLoginPassword) = $event),
                                name: "password",
                                type: "password",
                                label: _ctx.labels.password
                              }, null, 8, ["modelValue", "onUpdate:modelValue", "label"]),
                              _createElementVNode("div", null, [
                                _createVNode(_component_q_btn, {
                                  unelevated: "",
                                  "no-caps": "",
                                  color: "primary",
                                  type: "submit",
                                  label: _ctx.labels.signIn
                                }, null, 8, ["label"]),
                                _createVNode(_component_q_btn, {
                                  flat: "",
                                  "no-caps": "",
                                  color: "negative",
                                  onClick: _withModifiers(_ctx.reLoginReload, ["prevent"]),
                                  label: _ctx.labels.reloadPage
                                }, null, 8, ["onClick", "label"])
                              ])
                            ]),
                            _: 1
                          }, 8, ["onSubmit"])
                        ]))
                  ]),
                  _: 1
                }, 8, ["modelValue", "onUpdate:modelValue", "title"]),
                _createVNode(_component_m_shell_palette),
                _createVNode(_component_m_shell_quick_create),
                _createVNode(_component_m_shell_keys),
                (_ctx.chatStatus)
                  ? (_openBlock(), _createBlock(_component_m_user_status_dialog, { key: 0 }))
                  : _createCommentVNode("", true)
              ], 64))
            }
        } },
        "m-radio": { hash: "a9ce2610", make: function (Vue) {
            const { toDisplayString: _toDisplayString, createTextVNode: _createTextVNode, resolveComponent: _resolveComponent, withCtx: _withCtx, openBlock: _openBlock, createBlock: _createBlock, createCommentVNode: _createCommentVNode, createVNode: _createVNode, mergeProps: _mergeProps } = Vue

            return function render(_ctx, _cache) {
              const _component_q_tooltip = _resolveComponent("q-tooltip")
              const _component_q_option_group = _resolveComponent("q-option-group")
              const _component_m_field = _resolveComponent("m-field")

              return (_openBlock(), _createBlock(_component_m_field, _mergeProps({
                borderless: "",
                class: "m-field-group",
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
                  _createVNode(_component_q_option_group, {
                    type: "radio",
                    inline: "",
                    dense: "",
                    size: "sm",
                    options: _ctx.options,
                    "model-value": _ctx.local,
                    "onUpdate:modelValue": _ctx.onInput,
                    name: _ctx.name,
                    id: _ctx.id,
                    disable: _ctx.disable
                  }, null, 8, ["options", "model-value", "onUpdate:modelValue", "name", "id", "disable"])
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
        "m-record-card": { hash: "a6c7966d", make: function (Vue) {
            const { resolveComponent: _resolveComponent, createVNode: _createVNode, withCtx: _withCtx, toDisplayString: _toDisplayString, createTextVNode: _createTextVNode, openBlock: _openBlock, createBlock: _createBlock, createCommentVNode: _createCommentVNode, createElementBlock: _createElementBlock, renderList: _renderList, Fragment: _Fragment, createElementVNode: _createElementVNode, normalizeClass: _normalizeClass } = Vue

            return function render(_ctx, _cache) {
              const _component_m_menu_icon = _resolveComponent("m-menu-icon")
              const _component_q_item_section = _resolveComponent("q-item-section")
              const _component_m_link = _resolveComponent("m-link")
              const _component_q_item_label = _resolveComponent("q-item-label")
              const _component_q_chip = _resolveComponent("q-chip")
              const _component_q_item = _resolveComponent("q-item")
              const _component_q_skeleton = _resolveComponent("q-skeleton")
              const _component_q_card = _resolveComponent("q-card")

              return (_openBlock(), _createBlock(_component_q_card, {
                flat: "",
                bordered: "",
                class: "m-record-card"
              }, {
                default: _withCtx(() => [
                  (_ctx.info)
                    ? (_openBlock(), _createBlock(_component_q_item, {
                        key: 0,
                        dense: _ctx.dense,
                        class: "m-record-card-item"
                      }, {
                        default: _withCtx(() => [
                          _createVNode(_component_q_item_section, {
                            avatar: "",
                            class: "m-record-card-icon"
                          }, {
                            default: _withCtx(() => [
                              _createVNode(_component_m_menu_icon, {
                                name: _ctx.info.icon,
                                size: "sm"
                              }, null, 8, ["name"])
                            ]),
                            _: 1
                          }),
                          _createVNode(_component_q_item_section, { class: "m-record-card-body" }, {
                            default: _withCtx(() => [
                              _createVNode(_component_q_item_label, { class: "ellipsis text-weight-medium" }, {
                                default: _withCtx(() => [
                                  (_ctx.info.url)
                                    ? (_openBlock(), _createBlock(_component_m_link, {
                                        key: 0,
                                        href: _ctx.info.url,
                                        class: "m-link-text"
                                      }, {
                                        default: _withCtx(() => [
                                          _createTextVNode(_toDisplayString(_ctx.info.title), 1)
                                        ]),
                                        _: 1
                                      }, 8, ["href"]))
                                    : (_openBlock(), _createElementBlock("span", { key: 1 }, _toDisplayString(_ctx.info.title), 1))
                                ]),
                                _: 1
                              }),
                              _createVNode(_component_q_item_label, {
                                caption: "",
                                class: "ellipsis"
                              }, {
                                default: _withCtx(() => [
                                  _createTextVNode(_toDisplayString(_ctx.info.typeLabel), 1),
                                  (_ctx.info.typeLabel && _ctx.info.id)
                                    ? (_openBlock(), _createElementBlock("span", { key: 0 }, " · "))
                                    : _createCommentVNode("", true),
                                  _createTextVNode(_toDisplayString(_ctx.info.id), 1)
                                ]),
                                _: 1
                              }),
                              (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.info.fields, (field) => {
                                return (_openBlock(), _createBlock(_component_q_item_label, {
                                  key: field.label,
                                  caption: "",
                                  class: "ellipsis"
                                }, {
                                  default: _withCtx(() => [
                                    _createElementVNode("span", { class: "m-muted" }, _toDisplayString(field.label), 1),
                                    _createTextVNode(" " + _toDisplayString(field.value), 1)
                                  ]),
                                  _: 2
                                }, 1024))
                              }), 128))
                            ]),
                            _: 1
                          }),
                          (_ctx.info.status)
                            ? (_openBlock(), _createBlock(_component_q_item_section, {
                                key: 0,
                                side: "",
                                top: ""
                              }, {
                                default: _withCtx(() => [
                                  _createVNode(_component_q_chip, {
                                    dense: "",
                                    square: "",
                                    class: _normalizeClass('m-tonal text-' + _ctx.info.status.color)
                                  }, {
                                    default: _withCtx(() => [
                                      _createTextVNode(_toDisplayString(_ctx.info.status.label), 1)
                                    ]),
                                    _: 1
                                  }, 8, ["class"])
                                ]),
                                _: 1
                              }))
                            : _createCommentVNode("", true)
                        ]),
                        _: 1
                      }, 8, ["dense"]))
                    : (_ctx.failed || !_ctx.requestKey)
                      ? (_openBlock(), _createBlock(_component_q_item, {
                          key: 1,
                          dense: _ctx.dense,
                          class: "m-record-card-item"
                        }, {
                          default: _withCtx(() => [
                            _createVNode(_component_q_item_section, { class: "m-muted" }, {
                              default: _withCtx(() => [
                                _createTextVNode(_toDisplayString(_ctx.recordEntity) + " " + _toDisplayString(_ctx.recordId), 1)
                              ]),
                              _: 1
                            })
                          ]),
                          _: 1
                        }, 8, ["dense"]))
                      : (_openBlock(), _createBlock(_component_q_item, {
                          key: 2,
                          dense: _ctx.dense,
                          class: "m-record-card-item"
                        }, {
                          default: _withCtx(() => [
                            _createVNode(_component_q_item_section, { avatar: "" }, {
                              default: _withCtx(() => [
                                _createVNode(_component_q_skeleton, {
                                  type: "QAvatar",
                                  size: "24px"
                                })
                              ]),
                              _: 1
                            }),
                            _createVNode(_component_q_item_section, null, {
                              default: _withCtx(() => [
                                _createVNode(_component_q_skeleton, {
                                  type: "text",
                                  width: "60%"
                                }),
                                _createVNode(_component_q_skeleton, {
                                  type: "text",
                                  width: "40%"
                                })
                              ]),
                              _: 1
                            })
                          ]),
                          _: 1
                        }, 8, ["dense"]))
                ]),
                _: 1
              }))
            }
        } },
        "m-record-files": { hash: "8482661e", make: function (Vue) {
            const { resolveComponent: _resolveComponent, createVNode: _createVNode, normalizeClass: _normalizeClass, openBlock: _openBlock, createElementBlock: _createElementBlock, createCommentVNode: _createCommentVNode } = Vue

            return function render(_ctx, _cache) {
              const _component_m_dynamic_container = _resolveComponent("m-dynamic-container")

              return (_ctx.loadUrl)
                ? (_openBlock(), _createElementBlock("div", {
                    key: 0,
                    class: _normalizeClass('m-record-panel m-record-panel-' + _ctx.kind)
                  }, [
                    _createVNode(_component_m_dynamic_container, {
                      ref: "container",
                      id: _ctx.containerId,
                      url: _ctx.loadUrl
                    }, null, 8, ["id", "url"])
                  ], 2))
                : _createCommentVNode("", true)
            }
        } },
        "m-rich-text": { hash: "a6b11655", make: function (Vue) {
            const { resolveComponent: _resolveComponent, createVNode: _createVNode, createElementVNode: _createElementVNode, openBlock: _openBlock, createElementBlock: _createElementBlock, createCommentVNode: _createCommentVNode, toDisplayString: _toDisplayString, renderList: _renderList, Fragment: _Fragment, createBlock: _createBlock, createTextVNode: _createTextVNode, withCtx: _withCtx, resolveDirective: _resolveDirective, withDirectives: _withDirectives, withModifiers: _withModifiers, normalizeClass: _normalizeClass, withKeys: _withKeys, normalizeStyle: _normalizeStyle, vShow: _vShow, Teleport: _Teleport } = Vue

            return function render(_ctx, _cache) {
              const _component_q_btn = _resolveComponent("q-btn")
              const _component_m_text_area = _resolveComponent("m-text-area")
              const _component_q_separator = _resolveComponent("q-separator")
              const _component_q_item_section = _resolveComponent("q-item-section")
              const _component_q_item = _resolveComponent("q-item")
              const _component_q_list = _resolveComponent("q-list")
              const _component_q_btn_dropdown = _resolveComponent("q-btn-dropdown")
              const _component_q_tooltip = _resolveComponent("q-tooltip")
              const _component_q_input = _resolveComponent("q-input")
              const _component_q_spinner_dots = _resolveComponent("q-spinner-dots")
              const _component_m_avatar = _resolveComponent("m-avatar")
              const _directive_close_popup = _resolveDirective("close-popup")

              return (_openBlock(), _createElementBlock("div", {
                class: _normalizeClass(["m-rich-text", {'m-rich-text-light': _ctx.light, 'm-rich-text-focused': _ctx.focused}]),
                "data-state": _ctx.state
              }, [
                (_ctx.state === 'failed' && !_ctx.collab)
                  ? (_openBlock(), _createElementBlock("div", { key: 0 }, [
                      _createElementVNode("div", { class: "row items-center q-mb-xs" }, [
                        _createVNode(_component_q_btn, {
                          dense: "",
                          flat: "",
                          "no-caps": "",
                          icon: "sym_o_refresh",
                          label: _ctx.t('Try the editor again'),
                          onClick: _ctx.retryEditor
                        }, null, 8, ["label", "onClick"])
                      ]),
                      _createVNode(_component_m_text_area, {
                        "editor-type": "html",
                        "model-value": _ctx.local,
                        "onUpdate:modelValue": _ctx.onInput,
                        name: _ctx.name,
                        id: _ctx.id,
                        label: _ctx.label,
                        form: _ctx.form,
                        disable: _ctx.disable,
                        readonly: _ctx.readonly,
                        tooltip: _ctx.tooltip
                      }, null, 8, ["model-value", "onUpdate:modelValue", "name", "id", "label", "form", "disable", "readonly", "tooltip"])
                    ]))
                  : (_openBlock(), _createElementBlock(_Fragment, { key: 1 }, [
                      (_ctx.label)
                        ? (_openBlock(), _createElementBlock("div", {
                            key: 0,
                            class: "text-caption m-muted m-rich-text-label"
                          }, _toDisplayString(_ctx.label), 1))
                        : _createCommentVNode("", true),
                      _createElementVNode("div", {
                        class: _normalizeClass(["m-rich-text-box", [_ctx.bg ? 'bg-' + _ctx.bg : '', _ctx.errorAttrs.error ? 'm-rich-text-error' : '']])
                      }, [
                        (!_ctx.readonly)
                          ? (_openBlock(), _createElementBlock("div", {
                              key: 0,
                              class: "m-rich-text-toolbar row items-center"
                            }, [
                              (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.buttons, (button) => {
                                return (_openBlock(), _createElementBlock(_Fragment, {
                                  key: button.key
                                }, [
                                  (button.separator)
                                    ? (_openBlock(), _createBlock(_component_q_separator, {
                                        key: 0,
                                        vertical: "",
                                        class: "q-mx-xs"
                                      }))
                                    : (button.key === 'block')
                                      ? (_openBlock(), _createBlock(_component_q_btn_dropdown, {
                                          key: 1,
                                          flat: "",
                                          dense: "",
                                          "no-caps": "",
                                          size: "sm",
                                          label: _ctx.t(_ctx.blockLabel),
                                          disable: !_ctx.editable || _ctx.isPlain,
                                          class: "m-rich-text-block",
                                          "content-class": "m-rich-text-menu"
                                        }, {
                                          default: _withCtx(() => [
                                            _createVNode(_component_q_list, { dense: "" }, {
                                              default: _withCtx(() => [
                                                (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.blocks, (block) => {
                                                  return _withDirectives((_openBlock(), _createBlock(_component_q_item, {
                                                    key: block.key,
                                                    clickable: "",
                                                    onClick: $event => (_ctx.run(block.key)),
                                                    active: _ctx.blockLabel === block.label
                                                  }, {
                                                    default: _withCtx(() => [
                                                      _createVNode(_component_q_item_section, null, {
                                                        default: _withCtx(() => [
                                                          _createTextVNode(_toDisplayString(_ctx.t(block.label)), 1)
                                                        ]),
                                                        _: 2
                                                      }, 1024)
                                                    ]),
                                                    _: 2
                                                  }, 1032, ["onClick", "active"])), [
                                                    [_directive_close_popup]
                                                  ])
                                                }), 128))
                                              ]),
                                              _: 1
                                            })
                                          ]),
                                          _: 1
                                        }, 8, ["label", "disable"]))
                                      : (button.key === 'table')
                                        ? (_openBlock(), _createBlock(_component_q_btn_dropdown, {
                                            key: 2,
                                            flat: "",
                                            dense: "",
                                            size: "sm",
                                            icon: "sym_o_table",
                                            disable: !_ctx.editable || _ctx.isPlain,
                                            class: "m-rich-text-table",
                                            "content-class": "m-rich-text-menu",
                                            "aria-label": _ctx.t('Table'),
                                            onMousedown: _withModifiers(() => {}, ["prevent"])
                                          }, {
                                            default: _withCtx(() => [
                                              _createVNode(_component_q_list, { dense: "" }, {
                                                default: _withCtx(() => [
                                                  (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.tableItems, (item) => {
                                                    return _withDirectives((_openBlock(), _createBlock(_component_q_item, {
                                                      key: item.key,
                                                      clickable: "",
                                                      onClick: $event => (_ctx.run(item.key))
                                                    }, {
                                                      default: _withCtx(() => [
                                                        _createVNode(_component_q_item_section, null, {
                                                          default: _withCtx(() => [
                                                            _createTextVNode(_toDisplayString(_ctx.t(item.label)), 1)
                                                          ]),
                                                          _: 2
                                                        }, 1024)
                                                      ]),
                                                      _: 2
                                                    }, 1032, ["onClick"])), [
                                                      [_directive_close_popup]
                                                    ])
                                                  }), 128))
                                                ]),
                                                _: 1
                                              })
                                            ]),
                                            _: 1
                                          }, 8, ["disable", "aria-label", "onMousedown"]))
                                        : (_openBlock(), _createBlock(_component_q_btn, {
                                            key: 3,
                                            flat: "",
                                            dense: "",
                                            size: "sm",
                                            icon: button.icon,
                                            "data-command": button.key,
                                            "aria-label": _ctx.t(button.label),
                                            color: (button.key === 'plain' ? _ctx.isPlain : _ctx.active[button.key]) ? 'primary' : undefined,
                                            class: _normalizeClass({'m-rich-text-on': button.key === 'plain' ? _ctx.isPlain : _ctx.active[button.key]}),
                                            disable: button.key === 'plain' ? (_ctx.disable || _ctx.state !== 'ready') : (!_ctx.editable || _ctx.isPlain),
                                            onMousedown: _withModifiers(() => {}, ["prevent"]),
                                            onClick: $event => (_ctx.run(button.key))
                                          }, {
                                            default: _withCtx(() => [
                                              _createVNode(_component_q_tooltip, null, {
                                                default: _withCtx(() => [
                                                  _createTextVNode(_toDisplayString(_ctx.t(button.label)), 1)
                                                ]),
                                                _: 2
                                              }, 1024)
                                            ]),
                                            _: 2
                                          }, 1032, ["icon", "data-command", "aria-label", "color", "class", "disable", "onMousedown", "onClick"]))
                                ], 64))
                              }), 128)),
                              (_ctx.textActions.length)
                                ? (_openBlock(), _createBlock(_component_q_btn_dropdown, {
                                    key: 0,
                                    flat: "",
                                    dense: "",
                                    size: "sm",
                                    icon: "sym_o_auto_awesome",
                                    disable: !_ctx.canEdit || (_ctx.state !== 'ready' && !_ctx.isPlain),
                                    class: "m-rich-text-assist",
                                    "content-class": "m-rich-text-menu",
                                    "data-assist-text-actions": "",
                                    "aria-label": _ctx.assistLabel,
                                    onMousedown: _withModifiers(() => {}, ["prevent"])
                                  }, {
                                    default: _withCtx(() => [
                                      _createVNode(_component_q_list, { dense: "" }, {
                                        default: _withCtx(() => [
                                          (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.textActions, (item) => {
                                            return _withDirectives((_openBlock(), _createBlock(_component_q_item, {
                                              key: item.key,
                                              clickable: "",
                                              "data-assist-text-action": item.key,
                                              onClick: $event => (_ctx.textAction(item))
                                            }, {
                                              default: _withCtx(() => [
                                                _createVNode(_component_q_item_section, null, {
                                                  default: _withCtx(() => [
                                                    _createTextVNode(_toDisplayString(item.label), 1)
                                                  ]),
                                                  _: 2
                                                }, 1024)
                                              ]),
                                              _: 2
                                            }, 1032, ["data-assist-text-action", "onClick"])), [
                                              [_directive_close_popup]
                                            ])
                                          }), 128))
                                        ]),
                                        _: 1
                                      })
                                    ]),
                                    _: 1
                                  }, 8, ["disable", "aria-label", "onMousedown"]))
                                : _createCommentVNode("", true)
                            ]))
                          : _createCommentVNode("", true),
                        _createElementVNode("input", {
                          ref: "imageFile",
                          type: "file",
                          accept: "image/png,image/gif,image/jpeg,image/webp",
                          style: {"display":"none"},
                          onChange: _ctx.onImageFile
                        }, null, 40, ["onChange"]),
                        (_ctx.asking)
                          ? (_openBlock(), _createElementBlock("div", {
                              key: 1,
                              class: "m-rich-text-ask row items-center no-wrap q-px-sm q-py-xs q-gutter-x-xs"
                            }, [
                              _createVNode(_component_q_input, {
                                ref: "askInput",
                                dense: "",
                                outlined: "",
                                "hide-bottom-space": "",
                                class: "col",
                                modelValue: _ctx.askValue,
                                "onUpdate:modelValue": $event => ((_ctx.askValue) = $event),
                                placeholder: _ctx.asking === 'image' ? 'https://…/picture.png' : 'https://…',
                                "aria-label": _ctx.t(_ctx.asking === 'image' ? 'Image address' : 'Link address'),
                                onKeydown: [
                                  _withKeys(_withModifiers(_ctx.applyAsk, ["prevent"]), ["enter"]),
                                  _withKeys(_withModifiers($event => (_ctx.asking = null), ["prevent","stop"]), ["esc"])
                                ]
                              }, null, 8, ["modelValue", "onUpdate:modelValue", "placeholder", "aria-label", "onKeydown"]),
                              (_ctx.asking === 'image')
                                ? (_openBlock(), _createBlock(_component_q_btn, {
                                    key: 0,
                                    dense: "",
                                    flat: "",
                                    "no-caps": "",
                                    icon: "sym_o_upload",
                                    label: _ctx.t('File'),
                                    onClick: _ctx.pickImage
                                  }, null, 8, ["label", "onClick"]))
                                : _createCommentVNode("", true),
                              _createVNode(_component_q_btn, {
                                dense: "",
                                unelevated: "",
                                "no-caps": "",
                                class: "m-btn-tonal",
                                label: _ctx.t('Apply'),
                                onClick: _ctx.applyAsk
                              }, null, 8, ["label", "onClick"]),
                              (_ctx.asking === 'link' && _ctx.active.link)
                                ? (_openBlock(), _createBlock(_component_q_btn, {
                                    key: 1,
                                    dense: "",
                                    flat: "",
                                    "no-caps": "",
                                    label: _ctx.t('Remove'),
                                    onClick: _ctx.removeLink
                                  }, null, 8, ["label", "onClick"]))
                                : _createCommentVNode("", true),
                              _createVNode(_component_q_btn, {
                                dense: "",
                                flat: "",
                                round: "",
                                size: "sm",
                                icon: "sym_o_close",
                                "aria-label": _ctx.t('Close'),
                                onClick: $event => (_ctx.asking = null)
                              }, null, 8, ["aria-label", "onClick"])
                            ]))
                          : _createCommentVNode("", true),
                        (_ctx.proposal)
                          ? (_openBlock(), _createElementBlock("div", {
                              key: 2,
                              class: "m-rich-text-proposal q-px-sm q-py-xs",
                              "data-assist-text-proposal": "",
                              "data-state": _ctx.proposal.state,
                              "data-action": _ctx.proposal.key
                            }, [
                              (_ctx.proposal.state === 'loading')
                                ? (_openBlock(), _createElementBlock("div", {
                                    key: 0,
                                    class: "row items-center text-caption m-muted"
                                  }, [
                                    _createVNode(_component_q_spinner_dots, {
                                      size: "16px",
                                      class: "q-mr-xs"
                                    }),
                                    _createTextVNode(_toDisplayString(_ctx.proposal.label), 1)
                                  ]))
                                : (_ctx.proposal.state === 'failed')
                                  ? (_openBlock(), _createElementBlock("div", {
                                      key: 1,
                                      class: "text-caption text-negative"
                                    }, _toDisplayString(_ctx.proposal.error), 1))
                                  : (_openBlock(), _createElementBlock("div", {
                                      key: 2,
                                      class: "m-rich-text-proposal-text"
                                    }, _toDisplayString(_ctx.proposal.text), 1)),
                              _createElementVNode("div", { class: "row q-gutter-x-xs q-mt-xs" }, [
                                (_ctx.proposal.state === 'ready')
                                  ? (_openBlock(), _createElementBlock(_Fragment, { key: 0 }, [
                                      _createVNode(_component_q_btn, {
                                        dense: "",
                                        unelevated: "",
                                        "no-caps": "",
                                        color: "primary",
                                        label: _ctx.proposal.labels.replace,
                                        "data-assist-text-apply": "replace",
                                        onClick: $event => (_ctx.useProposal('replace'))
                                      }, null, 8, ["label", "onClick"]),
                                      _createVNode(_component_q_btn, {
                                        dense: "",
                                        flat: "",
                                        "no-caps": "",
                                        label: _ctx.proposal.labels.insertBelow,
                                        "data-assist-text-apply": "insert",
                                        onClick: $event => (_ctx.useProposal('insert'))
                                      }, null, 8, ["label", "onClick"])
                                    ], 64))
                                  : _createCommentVNode("", true),
                                _createVNode(_component_q_btn, {
                                  dense: "",
                                  flat: "",
                                  "no-caps": "",
                                  label: _ctx.proposal.labels.discard,
                                  "data-assist-text-apply": "discard",
                                  onClick: $event => (_ctx.proposal = null)
                                }, null, 8, ["label", "onClick"])
                              ])
                            ], 8, ["data-state", "data-action"]))
                          : _createCommentVNode("", true),
                        (_ctx.isPlain)
                          ? (_openBlock(), _createElementBlock("textarea", {
                              key: 3,
                              ref: "plainArea",
                              id: _ctx.id ? _ctx.id + '_plain' : null,
                              class: "m-rich-text-plain",
                              style: _normalizeStyle({minHeight: _ctx.minHeight}),
                              value: _ctx.plainText,
                              readonly: _ctx.readonly,
                              disabled: _ctx.disable,
                              onInput: $event => (_ctx.onPlainInput($event.target.value)),
                              onKeydown: _ctx.plainKey,
                              onFocus: $event => {_ctx.plainFocused = true; _ctx.focused = true},
                              onBlur: $event => {_ctx.plainFocused = false; _ctx.focused = false}
                            }, null, 44, ["id", "value", "readonly", "disabled", "onInput", "onKeydown", "onFocus", "onBlur"]))
                          : _createCommentVNode("", true),
                        _withDirectives(_createElementVNode("div", {
                          ref: "content",
                          class: "m-rich-text-content",
                          style: _normalizeStyle({minHeight: _ctx.minHeight}),
                          onClick: _ctx.focusEnd
                        }, null, 12, ["onClick"]), [
                          [_vShow, !_ctx.isPlain]
                        ]),
                        (_ctx.state === 'loading')
                          ? (_openBlock(), _createElementBlock("div", {
                              key: 4,
                              class: "m-rich-text-loading m-muted text-caption q-px-sm"
                            }, _toDisplayString(_ctx.t('Loading the editor')), 1))
                          : _createCommentVNode("", true)
                      ], 2),
                      (_ctx.errorAttrs.error)
                        ? (_openBlock(), _createElementBlock("div", {
                            key: 1,
                            class: "text-negative text-caption q-px-sm"
                          }, _toDisplayString(_ctx.errorAttrs.errorMessage), 1))
                        : _createCommentVNode("", true),
                      _createElementVNode("input", {
                        type: "hidden",
                        name: _ctx.name,
                        value: _ctx.local,
                        form: _ctx.form
                      }, null, 8, ["name", "value", "form"]),
                      (_openBlock(), _createBlock(_Teleport, { to: "body" }, [
                        _withDirectives(_createElementVNode("div", {
                          ref: "mentionMenu",
                          class: "m-rich-text-mentions",
                          id: _ctx.uid + '-mentions',
                          role: "listbox"
                        }, [
                          _createVNode(_component_q_list, { dense: "" }, {
                            default: _withCtx(() => [
                              (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.mention.items, (item, index) => {
                                return (_openBlock(), _createBlock(_component_q_item, {
                                  key: item.id,
                                  clickable: "",
                                  active: index === _ctx.mention.index,
                                  role: "option",
                                  "aria-selected": index === _ctx.mention.index,
                                  onMousedown: _withModifiers(() => {}, ["prevent"]),
                                  onClick: $event => (_ctx.pickMention(index))
                                }, {
                                  default: _withCtx(() => [
                                    _createVNode(_component_q_item_section, {
                                      avatar: "",
                                      class: "m-rich-text-mention-avatar"
                                    }, {
                                      default: _withCtx(() => [
                                        _createVNode(_component_m_avatar, {
                                          name: item.label,
                                          image: item.image,
                                          size: "22px"
                                        }, null, 8, ["name", "image"])
                                      ]),
                                      _: 2
                                    }, 1024),
                                    _createVNode(_component_q_item_section, null, {
                                      default: _withCtx(() => [
                                        _createTextVNode(_toDisplayString(item.label), 1)
                                      ]),
                                      _: 2
                                    }, 1024)
                                  ]),
                                  _: 2
                                }, 1032, ["active", "aria-selected", "onMousedown", "onClick"]))
                              }), 128)),
                              (!_ctx.mention.items.length)
                                ? (_openBlock(), _createBlock(_component_q_item, { key: 0 }, {
                                    default: _withCtx(() => [
                                      _createVNode(_component_q_item_section, { class: "m-muted" }, {
                                        default: _withCtx(() => [
                                          _createTextVNode(_toDisplayString(_ctx.t(_ctx.mention.loading ? 'Searching' : 'No matches')), 1)
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
                        ], 8, ["id"]), [
                          [_vShow, _ctx.mention.open]
                        ])
                      ]))
                    ], 64))
              ], 10, ["data-state"]))
            }
        } },
        "m-screen-context": { hash: "fcdf7cd5", make: function (Vue) {
            const { openBlock: _openBlock, createElementBlock: _createElementBlock } = Vue

            return function render(_ctx, _cache) {
              return (_openBlock(), _createElementBlock("span", { style: {"display":"none"} }))
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
        "m-shell-assist": { hash: "e5a3a37", make: function (Vue) {
            const { openBlock: _openBlock, createElementBlock: _createElementBlock, createCommentVNode: _createCommentVNode, resolveComponent: _resolveComponent, createVNode: _createVNode, createBlock: _createBlock, toDisplayString: _toDisplayString, createTextVNode: _createTextVNode, withCtx: _withCtx, normalizeClass: _normalizeClass, resolveDirective: _resolveDirective, withDirectives: _withDirectives, createElementVNode: _createElementVNode, renderList: _renderList, Fragment: _Fragment } = Vue

            return function render(_ctx, _cache) {
              const _component_q_icon = _resolveComponent("q-icon")
              const _component_q_select = _resolveComponent("q-select")
              const _component_q_space = _resolveComponent("q-space")
              const _component_q_btn_toggle = _resolveComponent("q-btn-toggle")
              const _component_q_circular_progress = _resolveComponent("q-circular-progress")
              const _component_q_tooltip = _resolveComponent("q-tooltip")
              const _component_q_btn = _resolveComponent("q-btn")
              const _component_q_badge = _resolveComponent("q-badge")
              const _component_q_item_section = _resolveComponent("q-item-section")
              const _component_q_toggle = _resolveComponent("q-toggle")
              const _component_q_item = _resolveComponent("q-item")
              const _component_q_list = _resolveComponent("q-list")
              const _component_q_menu = _resolveComponent("q-menu")
              const _component_q_spinner_dots = _resolveComponent("q-spinner-dots")
              const _component_q_chip = _resolveComponent("q-chip")
              const _component_q_separator = _resolveComponent("q-separator")
              const _component_m_assist_history = _resolveComponent("m-assist-history")
              const _component_m_assist_tasks = _resolveComponent("m-assist-tasks")
              const _component_m_assist_memory = _resolveComponent("m-assist-memory")
              const _component_m_assist_skills = _resolveComponent("m-assist-skills")
              const _component_m_assist_skill_editor = _resolveComponent("m-assist-skill-editor")
              const _component_m_assist_thread = _resolveComponent("m-assist-thread")
              const _component_m_assist_card = _resolveComponent("m-assist-card")
              const _component_m_assist_composer = _resolveComponent("m-assist-composer")
              const _component_q_drawer = _resolveComponent("q-drawer")
              const _directive_close_popup = _resolveDirective("close-popup")

              return (_openBlock(), _createBlock(_component_q_drawer, {
                modelValue: _ctx.open,
                "onUpdate:modelValue": $event => ((_ctx.open) = $event),
                side: "right",
                bordered: "",
                persistent: "",
                width: _ctx.width,
                breakpoint: 0,
                overlay: _ctx.overlay,
                class: "m-assist",
                id: "m-assist-drawer",
                "data-assist-drawer": ""
              }, {
                default: _withCtx(() => [
                  (_ctx.s.mounted)
                    ? (_openBlock(), _createElementBlock("div", {
                        key: 0,
                        class: "m-assist-body column no-wrap fit",
                        onKeydown: _ctx.onKey
                      }, [
                        (!_ctx.phone)
                          ? (_openBlock(), _createElementBlock("div", {
                              key: 0,
                              class: "m-assist-resize",
                              onMousedown: _ctx.startResize,
                              "aria-hidden": "true"
                            }, null, 40, ["onMousedown"]))
                          : _createCommentVNode("", true),
                        _createElementVNode("div", { class: "row items-center no-wrap q-px-sm q-py-xs m-assist-header" }, [
                          _createVNode(_component_q_icon, {
                            name: "sym_o_auto_awesome",
                            size: "sm",
                            class: "q-mr-xs text-primary"
                          }),
                          (_ctx.agentOptions.length > 1)
                            ? (_openBlock(), _createBlock(_component_q_select, {
                                key: 0,
                                "model-value": _ctx.convo.agentId || _ctx.s.agentId,
                                "onUpdate:modelValue": _ctx.setAgent,
                                options: _ctx.agentOptions,
                                "emit-value": "",
                                "map-options": "",
                                dense: "",
                                borderless: "",
                                "options-dense": "",
                                class: "m-assist-agent",
                                for: "m-assist-agent",
                                "aria-label": _ctx.labels.agent
                              }, null, 8, ["model-value", "onUpdate:modelValue", "options", "aria-label"]))
                            : (_openBlock(), _createElementBlock("div", {
                                key: 1,
                                class: "text-subtitle2 text-weight-medium ellipsis"
                              }, _toDisplayString(_ctx.agent.name || _ctx.labels.assist), 1)),
                          _createVNode(_component_q_space),
                          (_ctx.tierOptions.length > 1)
                            ? (_openBlock(), _createBlock(_component_q_btn_toggle, {
                                key: 2,
                                "model-value": _ctx.s.tier || _ctx.tiers[0],
                                "onUpdate:modelValue": _ctx.setTier,
                                options: _ctx.tierOptions,
                                dense: "",
                                flat: "",
                                "no-caps": "",
                                size: "sm",
                                "toggle-color": "primary",
                                class: "m-assist-tier",
                                id: "m-assist-tier"
                              }, null, 8, ["model-value", "onUpdate:modelValue", "options"]))
                            : _createCommentVNode("", true),
                          (_ctx.meter)
                            ? (_openBlock(), _createElementBlock("div", {
                                key: 3,
                                class: "m-assist-context q-mx-xs",
                                id: "m-assist-context",
                                tabindex: "0",
                                "data-assist-context-level": _ctx.meter.level,
                                "data-percent": _ctx.meter.percent,
                                "aria-label": _ctx.meter.tooltip
                              }, [
                                _createVNode(_component_q_circular_progress, {
                                  value: _ctx.meter.value,
                                  size: "24px",
                                  thickness: 0.2,
                                  color: _ctx.meter.level === 'warning' ? 'warning' : 'primary',
                                  "track-color": "grey-5",
                                  "show-value": "",
                                  "font-size": "8px"
                                }, {
                                  default: _withCtx(() => [
                                    _createTextVNode(_toDisplayString(_ctx.meter.percent), 1)
                                  ]),
                                  _: 1
                                }, 8, ["value", "color"]),
                                _createVNode(_component_q_tooltip, null, {
                                  default: _withCtx(() => [
                                    _createTextVNode(_toDisplayString(_ctx.meter.tooltip), 1)
                                  ]),
                                  _: 1
                                })
                              ], 8, ["data-assist-context-level", "data-percent", "aria-label"]))
                            : _createCommentVNode("", true),
                          _createVNode(_component_q_btn, {
                            flat: "",
                            round: "",
                            dense: "",
                            icon: "sym_o_edit_square",
                            id: "m-assist-new",
                            "aria-label": _ctx.labels.newConversation,
                            onClick: _ctx.newConversation
                          }, {
                            default: _withCtx(() => [
                              _createVNode(_component_q_tooltip, null, {
                                default: _withCtx(() => [
                                  _createTextVNode(_toDisplayString(_ctx.labels.newConversation), 1)
                                ]),
                                _: 1
                              })
                            ]),
                            _: 1
                          }, 8, ["aria-label", "onClick"]),
                          _createVNode(_component_q_btn, {
                            flat: "",
                            round: "",
                            dense: "",
                            icon: "sym_o_history",
                            id: "m-assist-history",
                            class: _normalizeClass({'text-primary': _ctx.s.pane === 'history'}),
                            "aria-label": _ctx.labels.history,
                            onClick: $event => (_ctx.pane('history'))
                          }, {
                            default: _withCtx(() => [
                              _createVNode(_component_q_tooltip, null, {
                                default: _withCtx(() => [
                                  _createTextVNode(_toDisplayString(_ctx.labels.history), 1)
                                ]),
                                _: 1
                              })
                            ]),
                            _: 1
                          }, 8, ["class", "aria-label", "onClick"]),
                          _createVNode(_component_q_btn, {
                            flat: "",
                            round: "",
                            dense: "",
                            icon: "sym_o_checklist",
                            id: "m-assist-tasks",
                            class: _normalizeClass({'text-primary': _ctx.s.pane === 'tasks'}),
                            "aria-label": _ctx.labels.tasks,
                            onClick: $event => (_ctx.pane('tasks'))
                          }, {
                            default: _withCtx(() => [
                              (_ctx.s.taskCount)
                                ? (_openBlock(), _createBlock(_component_q_badge, {
                                    key: 0,
                                    floating: "",
                                    rounded: "",
                                    color: "negative"
                                  }, {
                                    default: _withCtx(() => [
                                      _createTextVNode(_toDisplayString(_ctx.s.taskCount), 1)
                                    ]),
                                    _: 1
                                  }))
                                : _createCommentVNode("", true),
                              _createVNode(_component_q_tooltip, null, {
                                default: _withCtx(() => [
                                  _createTextVNode(_toDisplayString(_ctx.labels.tasks), 1)
                                ]),
                                _: 1
                              })
                            ]),
                            _: 1
                          }, 8, ["class", "aria-label", "onClick"]),
                          _createVNode(_component_q_btn, {
                            flat: "",
                            round: "",
                            dense: "",
                            icon: "sym_o_more_vert",
                            id: "m-assist-menu",
                            "aria-label": _ctx.labels.menu
                          }, {
                            default: _withCtx(() => [
                              _createVNode(_component_q_menu, {
                                anchor: "bottom right",
                                self: "top right"
                              }, {
                                default: _withCtx(() => [
                                  _createVNode(_component_q_list, {
                                    dense: "",
                                    style: {"min-width":"220px"}
                                  }, {
                                    default: _withCtx(() => [
                                      _createVNode(_component_q_item, {
                                        tag: "label",
                                        clickable: "",
                                        id: "m-assist-follow-toggle"
                                      }, {
                                        default: _withCtx(() => [
                                          _createVNode(_component_q_item_section, null, {
                                            default: _withCtx(() => [
                                              _createTextVNode(_toDisplayString(_ctx.labels.followOn), 1)
                                            ]),
                                            _: 1
                                          }),
                                          _createVNode(_component_q_item_section, { side: "" }, {
                                            default: _withCtx(() => [
                                              _createVNode(_component_q_toggle, {
                                                dense: "",
                                                "model-value": _ctx.s.followOn,
                                                "onUpdate:modelValue": _ctx.toggleFollow
                                              }, null, 8, ["model-value", "onUpdate:modelValue"])
                                            ]),
                                            _: 1
                                          })
                                        ]),
                                        _: 1
                                      }),
                                      (_ctx.memoryOn)
                                        ? (_openBlock(), _createBlock(_component_q_item, {
                                            key: 0,
                                            tag: "label",
                                            clickable: "",
                                            id: "m-assist-remember-toggle"
                                          }, {
                                            default: _withCtx(() => [
                                              _createVNode(_component_q_item_section, null, {
                                                default: _withCtx(() => [
                                                  _createTextVNode(_toDisplayString(_ctx.labels.remember), 1)
                                                ]),
                                                _: 1
                                              }),
                                              _createVNode(_component_q_item_section, { side: "" }, {
                                                default: _withCtx(() => [
                                                  _createVNode(_component_q_toggle, {
                                                    dense: "",
                                                    "model-value": _ctx.memoryGate.preference,
                                                    "onUpdate:modelValue": _ctx.setRemember
                                                  }, null, 8, ["model-value", "onUpdate:modelValue"])
                                                ]),
                                                _: 1
                                              })
                                            ]),
                                            _: 1
                                          }))
                                        : _createCommentVNode("", true),
                                      (_ctx.memoryOn)
                                        ? _withDirectives((_openBlock(), _createBlock(_component_q_item, {
                                            key: 1,
                                            clickable: "",
                                            id: "m-assist-memory",
                                            active: _ctx.s.pane === 'memory',
                                            onClick: $event => (_ctx.pane('memory'))
                                          }, {
                                            default: _withCtx(() => [
                                              _createVNode(_component_q_item_section, null, {
                                                default: _withCtx(() => [
                                                  _createTextVNode(_toDisplayString(_ctx.labels.memory), 1)
                                                ]),
                                                _: 1
                                              })
                                            ]),
                                            _: 1
                                          }, 8, ["active", "onClick"])), [
                                            [_directive_close_popup]
                                          ])
                                        : _createCommentVNode("", true),
                                      (_ctx.skillsOn)
                                        ? _withDirectives((_openBlock(), _createBlock(_component_q_item, {
                                            key: 2,
                                            clickable: "",
                                            id: "m-assist-skills",
                                            active: _ctx.s.pane === 'skills',
                                            onClick: $event => (_ctx.pane('skills'))
                                          }, {
                                            default: _withCtx(() => [
                                              _createVNode(_component_q_item_section, null, {
                                                default: _withCtx(() => [
                                                  _createTextVNode(_toDisplayString(_ctx.labels.mySkills), 1)
                                                ]),
                                                _: 1
                                              })
                                            ]),
                                            _: 1
                                          }, 8, ["active", "onClick"])), [
                                            [_directive_close_popup]
                                          ])
                                        : _createCommentVNode("", true)
                                    ]),
                                    _: 1
                                  })
                                ]),
                                _: 1
                              })
                            ]),
                            _: 1
                          }, 8, ["aria-label"]),
                          _createVNode(_component_q_btn, {
                            flat: "",
                            round: "",
                            dense: "",
                            icon: "sym_o_close",
                            id: "m-assist-close",
                            "aria-label": _ctx.labels.close,
                            onClick: _ctx.close
                          }, null, 8, ["aria-label", "onClick"])
                        ]),
                        (_ctx.canvasHidden)
                          ? (_openBlock(), _createElementBlock("div", {
                              key: 1,
                              class: "q-px-sm q-pb-xs"
                            }, [
                              _createVNode(_component_q_btn, {
                                flat: "",
                                dense: "",
                                "no-caps": "",
                                size: "sm",
                                color: "primary",
                                icon: "sym_o_dashboard",
                                id: "m-assist-show-canvas",
                                label: _ctx.labels.showCanvas + ': ' + _ctx.convo.canvas.title,
                                onClick: _ctx.showCanvas
                              }, null, 8, ["label", "onClick"])
                            ]))
                          : _createCommentVNode("", true),
                        (_ctx.convo.status || _ctx.elsewhere || _ctx.convo.chips.length)
                          ? (_openBlock(), _createElementBlock("div", {
                              key: 2,
                              class: "q-px-sm q-pb-xs m-assist-subheader"
                            }, [
                              (_ctx.convo.status)
                                ? (_openBlock(), _createElementBlock("div", {
                                    key: 0,
                                    class: "row items-center no-wrap text-caption m-assist-status",
                                    id: "m-assist-status"
                                  }, [
                                    (!_ctx.paused)
                                      ? (_openBlock(), _createBlock(_component_q_spinner_dots, {
                                          key: 0,
                                          size: "14px",
                                          class: "q-mr-xs"
                                        }))
                                      : (_openBlock(), _createBlock(_component_q_icon, {
                                          key: 1,
                                          name: "sym_o_pending_actions",
                                          color: "warning",
                                          size: "xs",
                                          class: "q-mr-xs"
                                        })),
                                    _createTextVNode(_toDisplayString(_ctx.convo.status), 1)
                                  ]))
                                : _createCommentVNode("", true),
                              (_ctx.elsewhere)
                                ? (_openBlock(), _createElementBlock("div", {
                                    key: 1,
                                    class: "row items-center no-wrap text-caption",
                                    id: "m-assist-elsewhere",
                                    "data-assist-follow": "elsewhere"
                                  }, [
                                    _createTextVNode(_toDisplayString(_ctx.labels.followingElsewhere), 1),
                                    _createVNode(_component_q_space),
                                    _createVNode(_component_q_btn, {
                                      flat: "",
                                      dense: "",
                                      "no-caps": "",
                                      size: "sm",
                                      color: "primary",
                                      id: "m-assist-follow-here",
                                      label: _ctx.labels.followHere,
                                      onClick: _ctx.followHere
                                    }, null, 8, ["label", "onClick"])
                                  ]))
                                : _createCommentVNode("", true),
                              (_ctx.convo.chips.length)
                                ? (_openBlock(), _createElementBlock("div", {
                                    key: 2,
                                    class: "row q-gutter-xs",
                                    id: "m-assist-chips"
                                  }, [
                                    (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.convo.chips, (chip) => {
                                      return (_openBlock(), _createBlock(_component_q_chip, {
                                        key: chip.key,
                                        dense: "",
                                        square: "",
                                        size: "sm",
                                        clickable: chip.kind === 'navigate',
                                        "data-assist-chip": chip.kind,
                                        onClick: $event => (_ctx.showChip(chip))
                                      }, {
                                        default: _withCtx(() => [
                                          _createTextVNode(_toDisplayString(chip.kind === 'navigate' ? _ctx.labels.opened : _ctx.labels.filled) + " " + _toDisplayString(chip.label), 1),
                                          (chip.kind === 'navigate')
                                            ? (_openBlock(), _createElementBlock("span", {
                                                key: 0,
                                                class: "text-primary q-ml-xs"
                                              }, _toDisplayString(_ctx.labels.show), 1))
                                            : _createCommentVNode("", true)
                                        ]),
                                        _: 2
                                      }, 1032, ["clickable", "data-assist-chip", "onClick"]))
                                    }), 128))
                                  ]))
                                : _createCommentVNode("", true)
                            ]))
                          : _createCommentVNode("", true),
                        _createVNode(_component_q_separator),
                        (_ctx.s.pane === 'history')
                          ? (_openBlock(), _createBlock(_component_m_assist_history, {
                              key: 3,
                              class: "col"
                            }))
                          : (_ctx.s.pane === 'tasks')
                            ? (_openBlock(), _createBlock(_component_m_assist_tasks, {
                                key: 4,
                                class: "col"
                              }))
                            : (_ctx.s.pane === 'memory')
                              ? (_openBlock(), _createBlock(_component_m_assist_memory, {
                                  key: 5,
                                  class: "col"
                                }))
                              : (_ctx.s.pane === 'skills')
                                ? (_openBlock(), _createBlock(_component_m_assist_skills, {
                                    key: 6,
                                    class: "col"
                                  }))
                                : (_ctx.s.pane === 'skill' && _ctx.s.skillEditor)
                                  ? (_openBlock(), _createBlock(_component_m_assist_skill_editor, {
                                      key: 7,
                                      class: "col"
                                    }))
                                  : (_openBlock(), _createElementBlock(_Fragment, { key: 8 }, [
                                      _createVNode(_component_m_assist_thread, { class: "col" }),
                                      (_ctx.handoffs.length || _ctx.approvals.length || _ctx.questions.length)
                                        ? (_openBlock(), _createElementBlock("div", {
                                            key: 0,
                                            class: "m-assist-cards q-px-sm q-pt-xs"
                                          }, [
                                            (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.handoffs, (card) => {
                                              return (_openBlock(), _createBlock(_component_m_assist_card, {
                                                key: 'h' + card.handoffId,
                                                kind: "handoff",
                                                card: card
                                              }, null, 8, ["card"]))
                                            }), 128)),
                                            (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.questions, (card) => {
                                              return (_openBlock(), _createBlock(_component_m_assist_card, {
                                                key: 'q' + card.questionId,
                                                kind: "question",
                                                card: card
                                              }, null, 8, ["card"]))
                                            }), 128))
                                          ]))
                                        : _createCommentVNode("", true),
                                      _createVNode(_component_m_assist_composer)
                                    ], 64)),
                        _createElementVNode("div", {
                          class: "m-assist-announce",
                          "aria-live": "polite",
                          role: "status"
                        }, _toDisplayString(_ctx.s.announce), 1)
                      ], 40, ["onKeydown"]))
                    : _createCommentVNode("", true)
                ]),
                _: 1
              }, 8, ["modelValue", "onUpdate:modelValue", "width", "overlay"]))
            }
        } },
        "m-shell-keys": { hash: "7bb1d715", make: function (Vue) {
            const { renderList: _renderList, Fragment: _Fragment, openBlock: _openBlock, createElementBlock: _createElementBlock, toDisplayString: _toDisplayString, createElementVNode: _createElementVNode, createTextVNode: _createTextVNode, resolveComponent: _resolveComponent, withCtx: _withCtx, createVNode: _createVNode, createBlock: _createBlock } = Vue

            return function render(_ctx, _cache) {
              const _component_q_item_section = _resolveComponent("q-item-section")
              const _component_q_item = _resolveComponent("q-item")
              const _component_q_list = _resolveComponent("q-list")
              const _component_m_dialog = _resolveComponent("m-dialog")

              return (_openBlock(), _createBlock(_component_m_dialog, {
                "model-value": _ctx.$root.keysHelpShow,
                "onUpdate:modelValue": $event => (_ctx.$root.keysHelpShow = $event),
                id: "m-keys-help",
                title: _ctx.$root.labels.keyboardShortcuts,
                width: "520"
              }, {
                default: _withCtx(() => [
                  (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.groups, (group) => {
                    return (_openBlock(), _createElementBlock("div", {
                      key: group.scope,
                      class: "q-mb-md"
                    }, [
                      _createElementVNode("div", { class: "text-subtitle2 q-mb-xs" }, _toDisplayString(group.title), 1),
                      _createVNode(_component_q_list, {
                        dense: "",
                        bordered: "",
                        separator: "",
                        class: "rounded-borders"
                      }, {
                        default: _withCtx(() => [
                          (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(group.entries, (entry) => {
                            return (_openBlock(), _createBlock(_component_q_item, {
                              key: entry.keys
                            }, {
                              default: _withCtx(() => [
                                _createVNode(_component_q_item_section, null, {
                                  default: _withCtx(() => [
                                    _createTextVNode(_toDisplayString(entry.label), 1)
                                  ]),
                                  _: 2
                                }, 1024),
                                _createVNode(_component_q_item_section, { side: "" }, {
                                  default: _withCtx(() => [
                                    _createElementVNode("span", { class: "m-kbd" }, _toDisplayString(entry.keys), 1)
                                  ]),
                                  _: 2
                                }, 1024)
                              ]),
                              _: 2
                            }, 1024))
                          }), 128))
                        ]),
                        _: 2
                      }, 1024)
                    ]))
                  }), 128))
                ]),
                _: 1
              }, 8, ["model-value", "onUpdate:modelValue", "title"]))
            }
        } },
        "m-shell-palette": { hash: "e4f4e679", make: function (Vue) {
            const { resolveComponent: _resolveComponent, openBlock: _openBlock, createBlock: _createBlock, createCommentVNode: _createCommentVNode, withCtx: _withCtx, createVNode: _createVNode, createElementVNode: _createElementVNode, renderList: _renderList, Fragment: _Fragment, createElementBlock: _createElementBlock, toDisplayString: _toDisplayString, createTextVNode: _createTextVNode, normalizeClass: _normalizeClass } = Vue

            return function render(_ctx, _cache) {
              const _component_q_spinner = _resolveComponent("q-spinner")
              const _component_q_icon = _resolveComponent("q-icon")
              const _component_m_text_line = _resolveComponent("m-text-line")
              const _component_q_separator = _resolveComponent("q-separator")
              const _component_q_item_label = _resolveComponent("q-item-label")
              const _component_m_menu_icon = _resolveComponent("m-menu-icon")
              const _component_q_item_section = _resolveComponent("q-item-section")
              const _component_q_item = _resolveComponent("q-item")
              const _component_q_list = _resolveComponent("q-list")
              const _component_q_card = _resolveComponent("q-card")
              const _component_q_dialog = _resolveComponent("q-dialog")

              return (_openBlock(), _createBlock(_component_q_dialog, {
                "model-value": _ctx.$root.paletteOpen,
                "onUpdate:modelValue": $event => (_ctx.$root.paletteOpen = $event),
                position: "top",
                onShow: _ctx.onShow,
                onHide: _ctx.onHide
              }, {
                default: _withCtx(() => [
                  _createVNode(_component_q_card, {
                    id: "m-palette",
                    flat: "",
                    bordered: "",
                    class: "m-palette column no-wrap"
                  }, {
                    default: _withCtx(() => [
                      _createElementVNode("div", { class: "q-pa-sm" }, [
                        _createVNode(_component_m_text_line, {
                          modelValue: _ctx.text,
                          "onUpdate:modelValue": $event => ((_ctx.text) = $event),
                          id: "m-palette-input",
                          name: "paletteText",
                          placeholder: _ctx.$root.labels.paletteHint,
                          onKeydown: _ctx.onKey
                        }, {
                          append: _withCtx(() => [
                            (_ctx.searching)
                              ? (_openBlock(), _createBlock(_component_q_spinner, {
                                  key: 0,
                                  size: "xs",
                                  color: "grey-6"
                                }))
                              : (_openBlock(), _createBlock(_component_q_icon, {
                                  key: 1,
                                  name: "sym_o_search",
                                  size: "xs"
                                }))
                          ]),
                          _: 1
                        }, 8, ["modelValue", "onUpdate:modelValue", "placeholder", "onKeydown"])
                      ]),
                      _createVNode(_component_q_separator),
                      _createElementVNode("div", {
                        ref: "results",
                        class: "col scroll m-palette-results"
                      }, [
                        _createVNode(_component_q_list, {
                          dense: "",
                          class: "q-pb-sm"
                        }, {
                          default: _withCtx(() => [
                            (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.groups, (group) => {
                              return (_openBlock(), _createElementBlock(_Fragment, {
                                key: group.key
                              }, [
                                _createVNode(_component_q_item_label, {
                                  header: "",
                                  class: "q-pt-sm q-pb-xs text-weight-medium"
                                }, {
                                  default: _withCtx(() => [
                                    _createTextVNode(_toDisplayString(group.label), 1)
                                  ]),
                                  _: 2
                                }, 1024),
                                (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(group.rows, (row) => {
                                  return (_openBlock(), _createBlock(_component_q_item, {
                                    key: group.key + '_' + row.index,
                                    clickable: "",
                                    active: row.index === _ctx.selected,
                                    "active-class": "m-palette-active",
                                    "data-index": row.index,
                                    onClick: $event => (_ctx.open(row, $event)),
                                    onMousemove: $event => (_ctx.selected = row.index)
                                  }, {
                                    default: _withCtx(() => [
                                      _createVNode(_component_q_item_section, {
                                        avatar: "",
                                        class: "m-palette-icon"
                                      }, {
                                        default: _withCtx(() => [
                                          (row.icon || (row.image && row.imageType === 'icon'))
                                            ? (_openBlock(), _createBlock(_component_m_menu_icon, {
                                                key: 0,
                                                name: row.icon || row.image,
                                                size: "xs"
                                              }, null, 8, ["name"]))
                                            : (row.image)
                                              ? (_openBlock(), _createElementBlock("img", {
                                                  key: 1,
                                                  src: row.image,
                                                  alt: "",
                                                  width: "18",
                                                  class: "invertible"
                                                }, null, 8, ["src"]))
                                              : (_openBlock(), _createBlock(_component_q_icon, {
                                                  key: 2,
                                                  name: "sym_o_chevron_right",
                                                  size: "xs"
                                                }))
                                        ]),
                                        _: 2
                                      }, 1024),
                                      _createVNode(_component_q_item_section, null, {
                                        default: _withCtx(() => [
                                          _createVNode(_component_q_item_label, {
                                            lines: "1",
                                            class: _normalizeClass({'text-primary': row.more})
                                          }, {
                                            default: _withCtx(() => [
                                              _createTextVNode(_toDisplayString(row.title), 1)
                                            ]),
                                            _: 2
                                          }, 1032, ["class"]),
                                          (row.subtitle)
                                            ? (_openBlock(), _createBlock(_component_q_item_label, {
                                                key: 0,
                                                caption: "",
                                                lines: "1"
                                              }, {
                                                default: _withCtx(() => [
                                                  _createTextVNode(_toDisplayString(row.subtitle), 1)
                                                ]),
                                                _: 2
                                              }, 1024))
                                            : _createCommentVNode("", true)
                                        ]),
                                        _: 2
                                      }, 1024)
                                    ]),
                                    _: 2
                                  }, 1032, ["active", "data-index", "onClick", "onMousemove"]))
                                }), 128))
                              ], 64))
                            }), 128)),
                            (!_ctx.rows.length && !_ctx.searching)
                              ? (_openBlock(), _createBlock(_component_q_item, { key: 0 }, {
                                  default: _withCtx(() => [
                                    _createVNode(_component_q_item_section, { class: "m-muted" }, {
                                      default: _withCtx(() => [
                                        _createTextVNode(_toDisplayString(_ctx.query.text ? _ctx.$root.labels.noResults : _ctx.$root.labels.paletteEmpty), 1)
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
                      ], 512),
                      _createVNode(_component_q_separator, { class: "gt-xs" }),
                      _createElementVNode("div", { class: "row items-center q-px-sm q-py-xs text-caption m-muted gt-xs" }, _toDisplayString(_ctx.hint), 1)
                    ]),
                    _: 1
                  })
                ]),
                _: 1
              }, 8, ["model-value", "onUpdate:modelValue", "onShow", "onHide"]))
            }
        } },
        "m-shell-quick-create": { hash: "9b22e439", make: function (Vue) {
            const { resolveComponent: _resolveComponent, openBlock: _openBlock, createBlock: _createBlock, createCommentVNode: _createCommentVNode, withCtx: _withCtx } = Vue

            return function render(_ctx, _cache) {
              const _component_m_dynamic_container = _resolveComponent("m-dynamic-container")
              const _component_m_dialog = _resolveComponent("m-dialog")

              return (_openBlock(), _createBlock(_component_m_dialog, {
                "model-value": _ctx.$root.quickCreateShow,
                "onUpdate:modelValue": $event => (_ctx.$root.quickCreateShow = $event),
                id: "m-quick-create",
                title: _ctx.$root.quickCreateTitle,
                width: "560",
                onOnShow: _ctx.onShow
              }, {
                default: _withCtx(() => [
                  (_ctx.$root.quickCreateShow && _ctx.$root.quickCreateUrl)
                    ? (_openBlock(), _createBlock(_component_m_dynamic_container, {
                        key: 0,
                        id: "m-quick-create-body",
                        url: _ctx.$root.quickCreateUrl
                      }, null, 8, ["url"]))
                    : _createCommentVNode("", true)
                ]),
                _: 1
              }, 8, ["model-value", "onUpdate:modelValue", "title", "onOnShow"]))
            }
        } },
        "m-shell-tray": { hash: "613fab4f", make: function (Vue) {
            const { renderList: _renderList, Fragment: _Fragment, openBlock: _openBlock, createElementBlock: _createElementBlock, toDisplayString: _toDisplayString, createTextVNode: _createTextVNode, resolveComponent: _resolveComponent, withCtx: _withCtx, createBlock: _createBlock, createCommentVNode: _createCommentVNode, createVNode: _createVNode, createElementVNode: _createElementVNode, vShow: _vShow, withDirectives: _withDirectives } = Vue

            return function render(_ctx, _cache) {
              const _component_q_badge = _resolveComponent("q-badge")
              const _component_q_tab = _resolveComponent("q-tab")
              const _component_q_tabs = _resolveComponent("q-tabs")
              const _component_q_tooltip = _resolveComponent("q-tooltip")
              const _component_q_btn = _resolveComponent("q-btn")
              const _component_q_separator = _resolveComponent("q-separator")
              const _component_m_dynamic_container = _resolveComponent("m-dynamic-container")
              const _component_q_card = _resolveComponent("q-card")
              const _component_q_page_sticky = _resolveComponent("q-page-sticky")

              return (_ctx.$root.trayMounted)
                ? _withDirectives((_openBlock(), _createBlock(_component_q_page_sticky, {
                    key: 0,
                    position: "right",
                    expand: "",
                    class: "m-tray-sticky"
                  }, {
                    default: _withCtx(() => [
                      _createVNode(_component_q_card, {
                        id: "m-tray",
                        flat: "",
                        bordered: "",
                        square: "",
                        class: "m-tray column no-wrap"
                      }, {
                        default: _withCtx(() => [
                          _createElementVNode("div", { class: "row items-center no-wrap q-pl-xs" }, [
                            _createVNode(_component_q_tabs, {
                              "model-value": _ctx.$root.trayTab,
                              "onUpdate:modelValue": $event => (_ctx.$root.openTray($event)),
                              dense: "",
                              "no-caps": "",
                              "inline-label": "",
                              "narrow-indicator": "",
                              align: "left",
                              class: "col text-grey-7",
                              "active-color": "primary",
                              "indicator-color": "primary"
                            }, {
                              default: _withCtx(() => [
                                (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.$root.trayTabs, (tab) => {
                                  return (_openBlock(), _createBlock(_component_q_tab, {
                                    key: tab.key,
                                    name: tab.key,
                                    icon: tab.icon,
                                    label: tab.label
                                  }, {
                                    default: _withCtx(() => [
                                      (_ctx.$root.countOf(tab.key))
                                        ? (_openBlock(), _createBlock(_component_q_badge, {
                                            key: 0,
                                            color: "negative",
                                            rounded: "",
                                            floating: ""
                                          }, {
                                            default: _withCtx(() => [
                                              _createTextVNode(_toDisplayString(_ctx.$root.countText(tab.key)), 1)
                                            ]),
                                            _: 2
                                          }, 1024))
                                        : _createCommentVNode("", true)
                                    ]),
                                    _: 2
                                  }, 1032, ["name", "icon", "label"]))
                                }), 128))
                              ]),
                              _: 1
                            }, 8, ["model-value", "onUpdate:modelValue"]),
                            _createVNode(_component_q_btn, {
                              flat: "",
                              round: "",
                              dense: "",
                              icon: "sym_o_close",
                              onClick: $event => (_ctx.$root.closeTray())
                            }, {
                              default: _withCtx(() => [
                                _createVNode(_component_q_tooltip, null, {
                                  default: _withCtx(() => [
                                    _createTextVNode(_toDisplayString(_ctx.$root.labels.close), 1)
                                  ]),
                                  _: 1
                                })
                              ]),
                              _: 1
                            }, 8, ["onClick"])
                          ]),
                          _createVNode(_component_q_separator),
                          (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.$root.trayTabs, (tab) => {
                            return _withDirectives((_openBlock(), _createElementBlock("div", {
                              key: tab.key,
                              ref_for: true,
                              ref: (el) => { _ctx.bodies[tab.key] = el; },
                              class: "col scroll q-pa-sm m-tray-body",
                              onScroll: $event => (_ctx.onScroll(tab.key, $event))
                            }, [
                              (_ctx.$root.trayLoaded[tab.key])
                                ? (_openBlock(), _createBlock(_component_m_dynamic_container, {
                                    key: 0,
                                    ref_for: true,
                                    ref: (vm) => { _ctx.containers[tab.key] = vm; },
                                    id: 'm-tray-' + tab.key,
                                    url: tab.loadUrl
                                  }, null, 8, ["id", "url"]))
                                : _createCommentVNode("", true)
                            ], 40, ["onScroll"])), [
                              [_vShow, _ctx.$root.trayTab === tab.key]
                            ])
                          }), 128))
                        ]),
                        _: 1
                      })
                    ]),
                    _: 1
                  }, 512)), [
                    [_vShow, _ctx.$root.trayOpen]
                  ])
                : _createCommentVNode("", true)
            }
        } },
        "m-status-bar": { hash: "bbb0c182", make: function (Vue) {
            const { renderList: _renderList, Fragment: _Fragment, openBlock: _openBlock, createElementBlock: _createElementBlock, resolveComponent: _resolveComponent, createBlock: _createBlock, createCommentVNode: _createCommentVNode, toDisplayString: _toDisplayString, createTextVNode: _createTextVNode, mergeProps: _mergeProps, withCtx: _withCtx, createVNode: _createVNode, createElementVNode: _createElementVNode } = Vue

            return function render(_ctx, _cache) {
              const _component_q_icon = _resolveComponent("q-icon")
              const _component_q_chip = _resolveComponent("q-chip")
              const _component_q_btn = _resolveComponent("q-btn")
              const _component_m_form = _resolveComponent("m-form")

              return (_openBlock(), _createElementBlock("div", {
                class: "m-status-bar row items-center",
                id: _ctx.id,
                "data-status": _ctx.view.current ? _ctx.view.current.statusId : null
              }, [
                _createElementVNode("div", { class: "m-status-stages row items-center" }, [
                  (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.view.stages, (stage, index) => {
                    return (_openBlock(), _createElementBlock(_Fragment, {
                      key: stage.statusId + ':' + index
                    }, [
                      index
                        ? (_openBlock(), _createBlock(_component_q_icon, {
                            key: 0,
                            name: "sym_o_chevron_right",
                            size: "xs",
                            class: "m-muted m-status-sep"
                          }))
                        : _createCommentVNode("", true),
                      _createVNode(_component_q_chip, _mergeProps({
                        dense: "",
                        square: "",
                        ripple: false
                      }, { ref_for: true }, stage.chip, {
                        "data-stage": stage.statusId,
                        "data-state": stage.state
                      }), {
                        default: _withCtx(() => [
                          _createTextVNode(_toDisplayString(stage.label), 1)
                        ]),
                        _: 2
                      }, 1040, ["data-stage", "data-state"])
                    ], 64))
                  }), 128))
                ]),
                (_ctx.action && _ctx.view.buttons.length)
                  ? (_openBlock(), _createElementBlock("div", {
                      key: 0,
                      class: "m-status-actions row items-center"
                    }, [
                      (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.view.buttons, (item) => {
                        return (_openBlock(), _createBlock(_component_m_form, {
                          key: item.toStatusId,
                          action: _ctx.action,
                          "fields-initial": _ctx.fieldsOf(item),
                          class: "m-status-form"
                        }, {
                          default: _withCtx(() => [
                            _createVNode(_component_q_btn, _mergeProps({
                              dense: "",
                              "no-caps": "",
                              unelevated: "",
                              type: "submit"
                            }, { ref_for: true }, item.button, {
                              label: item.label,
                              "data-confirm": item.confirm,
                              "data-to-status": item.toStatusId
                            }), null, 16, ["label", "data-confirm", "data-to-status"])
                          ]),
                          _: 2
                        }, 1032, ["action", "fields-initial"]))
                      }), 128))
                    ]))
                  : _createCommentVNode("", true)
              ], 8, ["id", "data-status"]))
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
        "m-subscreens-menu": { hash: "7c3c0f81", make: function (Vue) {
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
                      "narrow-indicator": "",
                      "inline-label": "",
                      class: "text-grey-7",
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
        "m-subscreens-stack": { hash: "88266d7b", make: function (Vue) {
            const { resolveComponent: _resolveComponent, createVNode: _createVNode, withCtx: _withCtx, openBlock: _openBlock, createBlock: _createBlock, createCommentVNode: _createCommentVNode, createElementBlock: _createElementBlock } = Vue

            return function render(_ctx, _cache) {
              const _component_q_breadcrumbs_el = _resolveComponent("q-breadcrumbs-el")
              const _component_q_breadcrumbs = _resolveComponent("q-breadcrumbs")
              const _component_m_subscreens_active = _resolveComponent("m-subscreens-active")

              return (_openBlock(), _createElementBlock("div", {
                class: "m-stack",
                id: _ctx.id
              }, [
                (_ctx.stacked)
                  ? (_openBlock(), _createBlock(_component_q_breadcrumbs, {
                      key: 0,
                      class: "m-stack-crumbs q-mb-sm",
                      "active-color": "grey-8"
                    }, {
                      default: _withCtx(() => [
                        _createVNode(_component_q_breadcrumbs_el, {
                          icon: "arrow_back",
                          label: _ctx.base.title,
                          class: "cursor-pointer text-primary",
                          onClick: $event => (_ctx.goTo(_ctx.base))
                        }, null, 8, ["label", "onClick"]),
                        _createVNode(_component_q_breadcrumbs_el, {
                          label: _ctx.active.title,
                          class: "text-weight-medium"
                        }, null, 8, ["label"])
                      ]),
                      _: 1
                    }))
                  : _createCommentVNode("", true),
                _createVNode(_component_m_subscreens_active)
              ], 8, ["id"]))
            }
        } },
        "m-subscreens-wizard": { hash: "b8c5e0fa", make: function (Vue) {
            const { renderList: _renderList, Fragment: _Fragment, openBlock: _openBlock, createElementBlock: _createElementBlock, resolveComponent: _resolveComponent, createBlock: _createBlock, withCtx: _withCtx, createCommentVNode: _createCommentVNode, createVNode: _createVNode, toDisplayString: _toDisplayString, createElementVNode: _createElementVNode } = Vue

            return function render(_ctx, _cache) {
              const _component_q_step = _resolveComponent("q-step")
              const _component_q_stepper = _resolveComponent("q-stepper")
              const _component_m_subscreens_active = _resolveComponent("m-subscreens-active")
              const _component_q_spinner = _resolveComponent("q-spinner")
              const _component_q_inner_loading = _resolveComponent("q-inner-loading")
              const _component_q_btn = _resolveComponent("q-btn")
              const _component_q_space = _resolveComponent("q-space")

              return (_openBlock(), _createElementBlock("div", {
                class: "m-wizard",
                id: _ctx.id,
                "data-step": _ctx.activeName,
                "data-step-form": _ctx.hasForm ? 'true' : 'false',
                "data-forms": _ctx.forms.length
              }, [
                (_ctx.steps.length)
                  ? (_openBlock(), _createBlock(_component_q_stepper, {
                      key: 0,
                      "model-value": _ctx.activeName,
                      flat: "",
                      bordered: "",
                      "header-nav": "",
                      class: "m-wizard-stepper",
                      "active-color": "primary",
                      "done-color": "primary",
                      contracted: _ctx.$q.screen.lt.sm,
                      "onUpdate:modelValue": _ctx.headerTo
                    }, {
                      default: _withCtx(() => [
                        (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.steps, (step, index) => {
                          return (_openBlock(), _createBlock(_component_q_step, {
                            key: step.name,
                            name: step.name,
                            title: step.title,
                            done: index < _ctx.activeIndex,
                            "header-nav": index < _ctx.activeIndex,
                            prefix: step.imageType === 'icon' && step.image ? undefined : String(index + 1),
                            icon: step.imageType === 'icon' && step.image ? step.image : undefined,
                            "data-step": step.name
                          }, null, 8, ["name", "title", "done", "header-nav", "prefix", "icon", "data-step"]))
                        }), 128))
                      ]),
                      _: 1
                    }, 8, ["model-value", "contracted", "onUpdate:modelValue"]))
                  : _createCommentVNode("", true),
                _createElementVNode("div", {
                  ref: "body",
                  class: "m-wizard-body"
                }, [
                  _createVNode(_component_m_subscreens_active),
                  _createVNode(_component_q_inner_loading, {
                    showing: _ctx.submitting,
                    class: "m-wizard-working"
                  }, {
                    default: _withCtx(() => [
                      _createVNode(_component_q_spinner, {
                        color: "primary",
                        size: "40px"
                      }),
                      _createElementVNode("div", {
                        class: "text-caption m-muted q-mt-sm",
                        textContent: _toDisplayString(_ctx.workingLabel)
                      }, null, 8, ["textContent"])
                    ]),
                    _: 1
                  }, 8, ["showing"])
                ], 512),
                (_ctx.steps.length)
                  ? (_openBlock(), _createElementBlock("div", {
                      key: 1,
                      class: "m-wizard-nav row items-center q-gutter-sm"
                    }, [
                      (_ctx.activeIndex > 0)
                        ? (_openBlock(), _createBlock(_component_q_btn, {
                            key: 0,
                            unelevated: "",
                            "no-caps": "",
                            class: "m-btn-tonal m-wizard-back",
                            icon: "arrow_back",
                            label: _ctx.backLabel,
                            disable: _ctx.submitting,
                            onClick: _ctx.back
                          }, null, 8, ["label", "disable", "onClick"]))
                        : _createCommentVNode("", true),
                      _createVNode(_component_q_space),
                      (!_ctx.isLast || _ctx.hasForm)
                        ? (_openBlock(), _createBlock(_component_q_btn, {
                            key: 1,
                            unelevated: "",
                            "no-caps": "",
                            color: "primary",
                            class: "m-wizard-next",
                            "icon-right": _ctx.isLast ? 'check' : 'arrow_forward',
                            label: _ctx.isLast ? _ctx.finishLabel : _ctx.nextLabel,
                            loading: _ctx.submitting,
                            onClick: _ctx.next
                          }, null, 8, ["icon-right", "label", "loading", "onClick"]))
                        : _createCommentVNode("", true)
                    ]))
                  : _createCommentVNode("", true)
              ], 8, ["id", "data-step", "data-step-form", "data-forms"]))
            }
        } },
        "m-text-area": { hash: "e5ced575", make: function (Vue) {
            const { resolveComponent: _resolveComponent, openBlock: _openBlock, createBlock: _createBlock, createCommentVNode: _createCommentVNode, toDisplayString: _toDisplayString, createElementBlock: _createElementBlock, createVNode: _createVNode, createElementVNode: _createElementVNode, createTextVNode: _createTextVNode, withCtx: _withCtx, mergeProps: _mergeProps } = Vue

            return function render(_ctx, _cache) {
              const _component_m_rich_text = _resolveComponent("m-rich-text")
              const _component_q_editor = _resolveComponent("q-editor")
              const _component_q_tooltip = _resolveComponent("q-tooltip")
              const _component_q_btn = _resolveComponent("q-btn")
              const _component_q_input = _resolveComponent("q-input")

              return (_ctx.editorType === 'rich' || _ctx.editorType === 'rich-light')
                ? (_openBlock(), _createBlock(_component_m_rich_text, {
                    key: 0,
                    light: _ctx.editorType === 'rich-light',
                    "model-value": _ctx.local,
                    "onUpdate:modelValue": _ctx.onInput,
                    name: _ctx.name,
                    id: _ctx.id,
                    label: _ctx.label,
                    form: _ctx.form,
                    disable: _ctx.disable,
                    readonly: _ctx.readonly,
                    tooltip: _ctx.tooltip,
                    rows: _ctx.rows,
                    "mention-url": _ctx.mentionUrl,
                    "mention-parameters": _ctx.mentionParameters,
                    "record-url": _ctx.recordUrl,
                    "record-parameters": _ctx.recordParameters
                  }, null, 8, ["light", "model-value", "onUpdate:modelValue", "name", "id", "label", "form", "disable", "readonly", "tooltip", "rows", "mention-url", "mention-parameters", "record-url", "record-parameters"]))
                : (_ctx.editorType === 'html')
                  ? (_openBlock(), _createElementBlock("div", {
                      key: 1,
                      class: "m-text-area-editor"
                    }, [
                      (_ctx.label)
                        ? (_openBlock(), _createElementBlock("div", {
                            key: 0,
                            class: "text-caption text-grey-7"
                          }, _toDisplayString(_ctx.label), 1))
                        : _createCommentVNode("", true),
                      _createVNode(_component_q_editor, {
                        "model-value": _ctx.local,
                        "onUpdate:modelValue": _ctx.onInput,
                        toolbar: _ctx.toolbar,
                        dense: "",
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
                        key: 2,
                        class: "m-text-area-editor"
                      }, [
                        _createVNode(_component_q_input, _mergeProps({
                          type: "textarea",
                          dense: "",
                          outlined: "",
                          "stack-label": "",
                          "hide-bottom-space": "",
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
                              flat: "",
                              round: "",
                              dense: "",
                              size: "sm",
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
                        key: 3,
                        type: "textarea",
                        dense: "",
                        outlined: "",
                        "stack-label": "",
                        "hide-bottom-space": "",
                        "no-error-icon": "",
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
        "m-text-find": { hash: "77d978ad", make: function (Vue) {
            const { createTextVNode: _createTextVNode, resolveComponent: _resolveComponent, withCtx: _withCtx, createVNode: _createVNode, openBlock: _openBlock, createBlock: _createBlock, createCommentVNode: _createCommentVNode, createElementVNode: _createElementVNode, createSlots: _createSlots } = Vue

            return function render(_ctx, _cache) {
              const _component_q_tooltip = _resolveComponent("q-tooltip")
              const _component_q_select = _resolveComponent("q-select")
              const _component_q_toggle = _resolveComponent("q-toggle")
              const _component_q_menu = _resolveComponent("q-menu")
              const _component_q_btn = _resolveComponent("q-btn")
              const _component_m_text_line = _resolveComponent("m-text-line")

              return (_openBlock(), _createBlock(_component_m_text_line, {
                label: _ctx.label,
                name: _ctx.name,
                id: _ctx.id,
                size: _ctx.size,
                maxlength: _ctx.maxlength,
                form: _ctx.form,
                tooltip: _ctx.tooltip,
                modelValue: _ctx.fields[_ctx.name],
                "onUpdate:modelValue": $event => ((_ctx.fields[_ctx.name]) = $event)
              }, _createSlots({ _: 2 }, [
                (_ctx.hideOptions !== 'true')
                  ? {
                      name: "append",
                      fn: _withCtx(() => [
                        _createVNode(_component_q_btn, {
                          flat: "",
                          round: "",
                          dense: "",
                          size: "sm",
                          icon: "tune",
                          color: _ctx.optionsActive ? 'primary' : 'grey-6'
                        }, {
                          default: _withCtx(() => [
                            _createVNode(_component_q_tooltip, null, {
                              default: _withCtx(() => [
                                _createTextVNode("Find Options")
                              ]),
                              _: 1
                            }),
                            _createVNode(_component_q_menu, { class: "m-text-find-options q-pa-sm" }, {
                              default: _withCtx(() => [
                                _createElementVNode("div", { class: "column q-gutter-y-xs" }, [
                                  (_ctx.hideOptions !== 'operator')
                                    ? (_openBlock(), _createBlock(_component_q_select, {
                                        key: 0,
                                        dense: "",
                                        outlined: "",
                                        "options-dense": "",
                                        "emit-value": "",
                                        "map-options": "",
                                        "stack-label": "",
                                        "hide-bottom-space": "",
                                        label: "Operator",
                                        name: _ctx.name + '_op',
                                        modelValue: _ctx.fields[_ctx.name + '_op'],
                                        "onUpdate:modelValue": $event => ((_ctx.fields[_ctx.name + '_op']) = $event),
                                        options: _ctx.operators,
                                        form: _ctx.form,
                                        behavior: "menu"
                                      }, null, 8, ["name", "modelValue", "onUpdate:modelValue", "options", "form"]))
                                    : _createCommentVNode("", true),
                                  (_ctx.hideOptions !== 'operator')
                                    ? (_openBlock(), _createBlock(_component_q_toggle, {
                                        key: 1,
                                        dense: "",
                                        size: "sm",
                                        name: _ctx.name + '_not',
                                        label: "Not",
                                        "true-value": "Y",
                                        "false-value": "N",
                                        modelValue: _ctx.fields[_ctx.name + '_not'],
                                        "onUpdate:modelValue": $event => ((_ctx.fields[_ctx.name + '_not']) = $event),
                                        form: _ctx.form
                                      }, null, 8, ["name", "modelValue", "onUpdate:modelValue", "form"]))
                                    : _createCommentVNode("", true),
                                  (_ctx.hideOptions !== 'ignore-case')
                                    ? (_openBlock(), _createBlock(_component_q_toggle, {
                                        key: 2,
                                        dense: "",
                                        size: "sm",
                                        name: _ctx.name + '_ic',
                                        label: "Ignore Case",
                                        "true-value": "Y",
                                        "false-value": "N",
                                        modelValue: _ctx.fields[_ctx.name + '_ic'],
                                        "onUpdate:modelValue": $event => ((_ctx.fields[_ctx.name + '_ic']) = $event),
                                        form: _ctx.form
                                      }, null, 8, ["name", "modelValue", "onUpdate:modelValue", "form"]))
                                    : _createCommentVNode("", true)
                                ])
                              ]),
                              _: 1
                            })
                          ]),
                          _: 1
                        }, 8, ["color"])
                      ]),
                      key: "0"
                    }
                  : undefined
              ]), 1032, ["label", "name", "id", "size", "maxlength", "form", "tooltip", "modelValue", "onUpdate:modelValue"]))
            }
        } },
        "m-text-line": { hash: "fdb70647", make: function (Vue) {
            const { toDisplayString: _toDisplayString, createTextVNode: _createTextVNode, resolveComponent: _resolveComponent, withCtx: _withCtx, openBlock: _openBlock, createBlock: _createBlock, createCommentVNode: _createCommentVNode, renderSlot: _renderSlot, mergeProps: _mergeProps, createSlots: _createSlots } = Vue

            return function render(_ctx, _cache) {
              const _component_q_tooltip = _resolveComponent("q-tooltip")
              const _component_q_input = _resolveComponent("q-input")

              return (_openBlock(), _createBlock(_component_q_input, _mergeProps({
                dense: "",
                outlined: "",
                "stack-label": "",
                "hide-bottom-space": "",
                "no-error-icon": "",
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
                autocomplete: _ctx.autocomplete
              }, _ctx.errorAttrs), _createSlots({
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
                (_ctx.$slots.append)
                  ? {
                      name: "append",
                      fn: _withCtx(() => [
                        _renderSlot(_ctx.$slots, "append")
                      ]),
                      key: "0"
                    }
                  : undefined
              ]), 1040, ["label", "prefix", "model-value", "onUpdate:modelValue", "type", "for", "name", "size", "maxlength", "form", "loading", "rules", "disable", "bg-color", "mask", "fill-mask", "reverse-fill-mask", "autocomplete"]))
            }
        } },
        "m-tree-item": { hash: "f60fcdc3", make: function (Vue) {
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
                      (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.children, (child) => {
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
        "m-tree-lead": { hash: "9cdbbe6a", make: function (Vue) {
            const { normalizeStyle: _normalizeStyle, createElementVNode: _createElementVNode, toDisplayString: _toDisplayString, openBlock: _openBlock, createElementBlock: _createElementBlock, createCommentVNode: _createCommentVNode, resolveComponent: _resolveComponent, withModifiers: _withModifiers, normalizeClass: _normalizeClass, createBlock: _createBlock } = Vue

            return function render(_ctx, _cache) {
              const _component_q_btn = _resolveComponent("q-btn")

              return (_ctx.t)
                ? (_openBlock(), _createElementBlock("span", {
                    key: 0,
                    class: "m-tree-lead row no-wrap items-center"
                  }, [
                    _createElementVNode("span", {
                      class: "m-tree-indent",
                      style: _normalizeStyle({width: _ctx.t.indent + 'px'})
                    }, null, 4),
                    (_ctx.t.deep)
                      ? (_openBlock(), _createElementBlock("span", {
                          key: 0,
                          class: "m-tree-depth m-muted text-caption"
                        }, _toDisplayString(_ctx.t.level), 1))
                      : _createCommentVNode("", true),
                    (_ctx.t.parent)
                      ? (_openBlock(), _createBlock(_component_q_btn, {
                          key: 1,
                          flat: "",
                          dense: "",
                          round: "",
                          size: "sm",
                          tabindex: "-1",
                          icon: "sym_o_chevron_right",
                          class: _normalizeClass(["m-tree-chevron", {'m-tree-chevron--open': _ctx.t.open}]),
                          loading: _ctx.t.loading,
                          "aria-label": _ctx.t.open ? _ctx.t.closeLabel : _ctx.t.openLabel,
                          onClick: _withModifiers($event => (_ctx.t.toggle()), ["stop"])
                        }, null, 8, ["class", "loading", "aria-label", "onClick"]))
                      : (_openBlock(), _createElementBlock("span", {
                          key: 2,
                          class: "m-tree-gutter"
                        }))
                  ]))
                : _createCommentVNode("", true)
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
        } },
        "m-user-status-dialog": { hash: "8b5ac844", make: function (Vue) {
            const { toDisplayString: _toDisplayString, createTextVNode: _createTextVNode, resolveComponent: _resolveComponent, withCtx: _withCtx, createVNode: _createVNode, renderList: _renderList, Fragment: _Fragment, openBlock: _openBlock, createElementBlock: _createElementBlock, createBlock: _createBlock, createCommentVNode: _createCommentVNode, withKeys: _withKeys, createElementVNode: _createElementVNode, resolveDirective: _resolveDirective, withDirectives: _withDirectives } = Vue

            return function render(_ctx, _cache) {
              const _component_q_card_section = _resolveComponent("q-card-section")
              const _component_q_item_section = _resolveComponent("q-item-section")
              const _component_q_item = _resolveComponent("q-item")
              const _component_q_list = _resolveComponent("q-list")
              const _component_q_input = _resolveComponent("q-input")
              const _component_q_select = _resolveComponent("q-select")
              const _component_q_btn = _resolveComponent("q-btn")
              const _component_q_card_actions = _resolveComponent("q-card-actions")
              const _component_q_card = _resolveComponent("q-card")
              const _component_q_dialog = _resolveComponent("q-dialog")
              const _directive_close_popup = _resolveDirective("close-popup")

              return (_openBlock(), _createBlock(_component_q_dialog, {
                modelValue: _ctx.open,
                "onUpdate:modelValue": $event => ((_ctx.open) = $event)
              }, {
                default: _withCtx(() => [
                  _createVNode(_component_q_card, { style: {"min-width":"320px","max-width":"96vw"} }, {
                    default: _withCtx(() => [
                      _createVNode(_component_q_card_section, { class: "text-subtitle1" }, {
                        default: _withCtx(() => [
                          _createTextVNode(_toDisplayString(_ctx.labels.setStatus), 1)
                        ]),
                        _: 1
                      }),
                      _createVNode(_component_q_card_section, { class: "q-pt-none q-gutter-y-sm" }, {
                        default: _withCtx(() => [
                          (_ctx.presets.length)
                            ? (_openBlock(), _createBlock(_component_q_list, {
                                key: 0,
                                dense: "",
                                class: "m-status-presets"
                              }, {
                                default: _withCtx(() => [
                                  (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.presets, (preset, index) => {
                                    return (_openBlock(), _createBlock(_component_q_item, {
                                      key: index,
                                      clickable: "",
                                      dense: "",
                                      "data-preset": index,
                                      onClick: $event => (_ctx.usePreset(preset))
                                    }, {
                                      default: _withCtx(() => [
                                        _createVNode(_component_q_item_section, {
                                          avatar: "",
                                          style: {"min-width":"28px"}
                                        }, {
                                          default: _withCtx(() => [
                                            _createTextVNode(_toDisplayString(preset.emoji), 1)
                                          ]),
                                          _: 2
                                        }, 1024),
                                        _createVNode(_component_q_item_section, null, {
                                          default: _withCtx(() => [
                                            _createTextVNode(_toDisplayString(preset.text), 1)
                                          ]),
                                          _: 2
                                        }, 1024),
                                        _createVNode(_component_q_item_section, {
                                          side: "",
                                          class: "text-caption"
                                        }, {
                                          default: _withCtx(() => [
                                            _createTextVNode(_toDisplayString(_ctx.clearLabel(preset.clearAfter)), 1)
                                          ]),
                                          _: 2
                                        }, 1024)
                                      ]),
                                      _: 2
                                    }, 1032, ["data-preset", "onClick"]))
                                  }), 128))
                                ]),
                                _: 1
                              }))
                            : _createCommentVNode("", true),
                          _createElementVNode("div", { class: "row no-wrap q-gutter-x-sm" }, [
                            _createVNode(_component_q_input, {
                              modelValue: _ctx.emoji,
                              "onUpdate:modelValue": $event => ((_ctx.emoji) = $event),
                              dense: "",
                              outlined: "",
                              "stack-label": "",
                              label: _ctx.labels.emoji,
                              maxlength: "16",
                              style: {"width":"84px"},
                              name: "emoji"
                            }, null, 8, ["modelValue", "onUpdate:modelValue", "label"]),
                            _createVNode(_component_q_input, {
                              modelValue: _ctx.text,
                              "onUpdate:modelValue": $event => ((_ctx.text) = $event),
                              dense: "",
                              outlined: "",
                              "stack-label": "",
                              autofocus: "",
                              class: "col",
                              label: _ctx.labels.statusText,
                              maxlength: "100",
                              name: "statusText",
                              onKeyup: _withKeys(_ctx.save, ["enter"])
                            }, null, 8, ["modelValue", "onUpdate:modelValue", "label", "onKeyup"])
                          ]),
                          _createVNode(_component_q_select, {
                            modelValue: _ctx.clear,
                            "onUpdate:modelValue": $event => ((_ctx.clear) = $event),
                            options: _ctx.clearOptions,
                            "emit-value": "",
                            "map-options": "",
                            dense: "",
                            outlined: "",
                            "stack-label": "",
                            label: _ctx.labels.clearAfter,
                            name: "clearAt"
                          }, null, 8, ["modelValue", "onUpdate:modelValue", "options", "label"])
                        ]),
                        _: 1
                      }),
                      _createVNode(_component_q_card_actions, { align: "right" }, {
                        default: _withCtx(() => [
                          _withDirectives(_createVNode(_component_q_btn, {
                            flat: "",
                            "no-caps": "",
                            label: _ctx.labels.cancel
                          }, null, 8, ["label"]), [
                            [_directive_close_popup]
                          ]),
                          _createVNode(_component_q_btn, {
                            unelevated: "",
                            "no-caps": "",
                            color: "primary",
                            label: _ctx.labels.save,
                            loading: _ctx.saving,
                            id: "m-status-save",
                            onClick: _ctx.save
                          }, null, 8, ["label", "loading", "onClick"])
                        ]),
                        _: 1
                      })
                    ]),
                    _: 1
                  })
                ]),
                _: 1
              }, 8, ["modelValue", "onUpdate:modelValue"]))
            }
        } },
        "m-user-status-menu": { hash: "8683de21", make: function (Vue) {
            const { normalizeClass: _normalizeClass, createElementVNode: _createElementVNode, resolveComponent: _resolveComponent, withCtx: _withCtx, createVNode: _createVNode, toDisplayString: _toDisplayString, createTextVNode: _createTextVNode, openBlock: _openBlock, createBlock: _createBlock, createCommentVNode: _createCommentVNode, renderList: _renderList, Fragment: _Fragment, createElementBlock: _createElementBlock, resolveDirective: _resolveDirective, withDirectives: _withDirectives } = Vue

            return function render(_ctx, _cache) {
              const _component_q_item_section = _resolveComponent("q-item-section")
              const _component_q_item_label = _resolveComponent("q-item-label")
              const _component_q_item = _resolveComponent("q-item")
              const _component_q_icon = _resolveComponent("q-icon")
              const _directive_close_popup = _resolveDirective("close-popup")

              return (_openBlock(), _createElementBlock("div", { class: "m-user-status" }, [
                _createVNode(_component_q_item, {
                  id: "m-status-current",
                  dense: ""
                }, {
                  default: _withCtx(() => [
                    _createVNode(_component_q_item_section, {
                      avatar: "",
                      class: "m-status-avatar"
                    }, {
                      default: _withCtx(() => [
                        _createElementVNode("span", {
                          class: _normalizeClass(["m-status-dot", 'm-presence-' + _ctx.currentDot])
                        }, null, 2)
                      ]),
                      _: 1
                    }),
                    _createVNode(_component_q_item_section, null, {
                      default: _withCtx(() => [
                        _createVNode(_component_q_item_label, null, {
                          default: _withCtx(() => [
                            _createTextVNode(_toDisplayString(_ctx.currentLabel), 1)
                          ]),
                          _: 1
                        }),
                        (_ctx.me.statusText || _ctx.me.emoji)
                          ? (_openBlock(), _createBlock(_component_q_item_label, {
                              key: 0,
                              caption: "",
                              class: "m-status-text"
                            }, {
                              default: _withCtx(() => [
                                _createTextVNode(_toDisplayString(_ctx.me.emoji ? _ctx.me.emoji + ' ' : '') + _toDisplayString(_ctx.me.statusText), 1)
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
                (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.options, (option) => {
                  return _withDirectives((_openBlock(), _createBlock(_component_q_item, {
                    key: option.value,
                    dense: "",
                    clickable: "",
                    active: _ctx.current === option.value,
                    "data-status": option.value,
                    onClick: $event => (_ctx.choose(option.value))
                  }, {
                    default: _withCtx(() => [
                      _createVNode(_component_q_item_section, {
                        avatar: "",
                        class: "m-status-avatar"
                      }, {
                        default: _withCtx(() => [
                          _createElementVNode("span", {
                            class: _normalizeClass(["m-status-dot", 'm-presence-' + option.dot])
                          }, null, 2)
                        ]),
                        _: 2
                      }, 1024),
                      _createVNode(_component_q_item_section, null, {
                        default: _withCtx(() => [
                          _createTextVNode(_toDisplayString(option.label), 1)
                        ]),
                        _: 2
                      }, 1024)
                    ]),
                    _: 2
                  }, 1032, ["active", "data-status", "onClick"])), [
                    [_directive_close_popup]
                  ])
                }), 128)),
                _withDirectives((_openBlock(), _createBlock(_component_q_item, {
                  dense: "",
                  clickable: "",
                  "data-status": "message",
                  onClick: _ctx.openDialog
                }, {
                  default: _withCtx(() => [
                    _createVNode(_component_q_item_section, {
                      avatar: "",
                      class: "m-status-avatar"
                    }, {
                      default: _withCtx(() => [
                        _createVNode(_component_q_icon, {
                          name: "sym_o_edit",
                          size: "xs"
                        })
                      ]),
                      _: 1
                    }),
                    _createVNode(_component_q_item_section, null, {
                      default: _withCtx(() => [
                        _createTextVNode(_toDisplayString(_ctx.labels.setStatus), 1)
                      ]),
                      _: 1
                    })
                  ]),
                  _: 1
                }, 8, ["onClick"])), [
                  [_directive_close_popup]
                ]),
                (_ctx.me.statusText || _ctx.me.emoji)
                  ? _withDirectives((_openBlock(), _createBlock(_component_q_item, {
                      key: 0,
                      dense: "",
                      clickable: "",
                      "data-status": "clear",
                      onClick: _ctx.clearText
                    }, {
                      default: _withCtx(() => [
                        _createVNode(_component_q_item_section, {
                          avatar: "",
                          class: "m-status-avatar"
                        }, {
                          default: _withCtx(() => [
                            _createVNode(_component_q_icon, {
                              name: "sym_o_close",
                              size: "xs"
                            })
                          ]),
                          _: 1
                        }),
                        _createVNode(_component_q_item_section, null, {
                          default: _withCtx(() => [
                            _createTextVNode(_toDisplayString(_ctx.labels.clearStatus), 1)
                          ]),
                          _: 1
                        })
                      ]),
                      _: 1
                    }, 8, ["onClick"])), [
                      [_directive_close_popup]
                    ])
                  : _createCommentVNode("", true)
              ]))
            }
        } }
    };
})(globalThis.moqui);

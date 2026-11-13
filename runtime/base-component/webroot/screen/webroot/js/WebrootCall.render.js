/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

// Generated from the component templates of WebrootCall.js; do not edit. Make it again with
//   UPDATE_GOLDEN=1 cargo test -p moqui-core --lib call_components_are_precompiled
// qvt.applyRenders uses an entry only while its hash matches the template (plans/archive/QvtServerStatic.md, Page build).
(function (moqui) {
    "use strict";
    moqui.qvtRenders = Object.assign(moqui.qvtRenders || {}, {
        "m-call": { hash: "2cd665ca", make: function (Vue) {
            const { toDisplayString: _toDisplayString, createElementVNode: _createElementVNode, createTextVNode: _createTextVNode, resolveComponent: _resolveComponent, withCtx: _withCtx, createVNode: _createVNode, openBlock: _openBlock, createBlock: _createBlock, createCommentVNode: _createCommentVNode, normalizeClass: _normalizeClass, createElementBlock: _createElementBlock, renderList: _renderList, Fragment: _Fragment, normalizeStyle: _normalizeStyle, resolveDirective: _resolveDirective, withDirectives: _withDirectives } = Vue

            return function render(_ctx, _cache) {
              const _component_q_chip = _resolveComponent("q-chip")
              const _component_q_tooltip = _resolveComponent("q-tooltip")
              const _component_q_space = _resolveComponent("q-space")
              const _component_q_banner = _resolveComponent("q-banner")
              const _component_q_icon = _resolveComponent("q-icon")
              const _component_q_btn = _resolveComponent("q-btn")
              const _component_m_call_context = _resolveComponent("m-call-context")
              const _component_m_call_video = _resolveComponent("m-call-video")
              const _component_q_linear_progress = _resolveComponent("q-linear-progress")
              const _component_q_select = _resolveComponent("q-select")
              const _component_q_toggle = _resolveComponent("q-toggle")
              const _component_q_menu = _resolveComponent("q-menu")
              const _component_q_badge = _resolveComponent("q-badge")
              const _component_m_call_actions = _resolveComponent("m-call-actions")
              const _component_m_ext_actions = _resolveComponent("m-ext-actions")
              const _component_m_avatar = _resolveComponent("m-avatar")
              const _component_q_item_section = _resolveComponent("q-item-section")
              const _component_q_item_label = _resolveComponent("q-item-label")
              const _component_q_item = _resolveComponent("q-item")
              const _component_q_list = _resolveComponent("q-list")
              const _directive_close_popup = _resolveDirective("close-popup")

              return (_openBlock(), _createElementBlock("div", { class: "m-call column q-gutter-sm" }, [
                _createElementVNode("div", { class: "row items-center" }, [
                  _createElementVNode("div", { class: "text-h6 q-mr-md" }, _toDisplayString(_ctx.known ? (_ctx.known.participants || []).filter(p => p.userId !== _ctx.moquiCallMe()).map(p => p.displayName).join(', ') || _ctx.t('Call') : _ctx.t('Call')), 1),
                  _createVNode(_component_q_chip, {
                    dense: "",
                    square: "",
                    color: _ctx.known && _ctx.known.status === 'ended' ? 'grey-6' : 'positive',
                    "text-color": "white"
                  }, {
                    default: _withCtx(() => [
                      _createTextVNode(_toDisplayString(_ctx.stateText), 1)
                    ]),
                    _: 1
                  }, 8, ["color"]),
                  (_ctx.recording)
                    ? (_openBlock(), _createBlock(_component_q_chip, {
                        key: 0,
                        dense: "",
                        square: "",
                        color: _ctx.recording.state === 'recording' ? 'negative' : 'warning',
                        "text-color": "white",
                        icon: "sym_o_fiber_manual_record",
                        class: "m-call-recording"
                      }, {
                        default: _withCtx(() => [
                          _createTextVNode(_toDisplayString(_ctx.recordingText), 1)
                        ]),
                        _: 1
                      }, 8, ["color"]))
                    : _createCommentVNode("", true),
                  (_ctx.known && _ctx.known.encryptionMode)
                    ? (_openBlock(), _createBlock(_component_q_chip, {
                        key: 1,
                        dense: "",
                        square: "",
                        class: _normalizeClass(_ctx.e2ee ? 'm-call-encryption m-call-e2ee m-tonal text-positive' : 'm-call-encryption m-tonal'),
                        icon: _ctx.e2ee ? 'sym_o_lock' : 'sym_o_shield'
                      }, {
                        default: _withCtx(() => [
                          _createTextVNode(_toDisplayString(_ctx.encryptionText), 1),
                          _createVNode(_component_q_tooltip, { "max-width": "320px" }, {
                            default: _withCtx(() => [
                              _createTextVNode(_toDisplayString(_ctx.encryptionTip), 1)
                            ]),
                            _: 1
                          })
                        ]),
                        _: 1
                      }, 8, ["class", "icon"]))
                    : _createCommentVNode("", true),
                  (_ctx.inThis)
                    ? (_openBlock(), _createElementBlock("span", {
                        key: 2,
                        class: "text-caption q-ml-sm"
                      }, _toDisplayString(_ctx.elapsed), 1))
                    : _createCommentVNode("", true),
                  _createVNode(_component_q_space),
                  (_ctx.inThis && _ctx.call.quality)
                    ? (_openBlock(), _createElementBlock("span", {
                        key: 3,
                        class: "text-caption m-muted"
                      }, _toDisplayString(_ctx.call.quality) + " kbps", 1))
                    : _createCommentVNode("", true)
                ]),
                (_ctx.call.error && _ctx.inThis)
                  ? (_openBlock(), _createBlock(_component_q_banner, {
                      key: 0,
                      dense: "",
                      rounded: "",
                      class: "m-tonal text-warning"
                    }, {
                      default: _withCtx(() => [
                        _createTextVNode(_toDisplayString(_ctx.call.error), 1)
                      ]),
                      _: 1
                    }))
                  : _createCommentVNode("", true),
                (_ctx.offline && !_ctx.inThis)
                  ? (_openBlock(), _createBlock(_component_q_banner, {
                      key: 1,
                      dense: "",
                      rounded: "",
                      class: "m-tonal text-warning m-call-offline"
                    }, {
                      avatar: _withCtx(() => [
                        _createVNode(_component_q_icon, { name: "sym_o_cloud_off" })
                      ]),
                      default: _withCtx(() => [
                        _createTextVNode(_toDisplayString(_ctx.offlineText), 1)
                      ]),
                      _: 1
                    }))
                  : _createCommentVNode("", true),
                (_ctx.keysInShell && !_ctx.inThis && _ctx.canJoin)
                  ? (_openBlock(), _createBlock(_component_q_banner, {
                      key: 2,
                      dense: "",
                      rounded: "",
                      class: "m-tonal m-call-in-shell"
                    }, {
                      avatar: _withCtx(() => [
                        _createVNode(_component_q_icon, { name: "sym_o_lock" })
                      ]),
                      action: _withCtx(() => [
                        _createVNode(_component_q_btn, {
                          flat: "",
                          "no-caps": "",
                          color: "primary",
                          icon: "sym_o_open_in_new",
                          label: _ctx.openInShellLabel,
                          onClick: _ctx.openInShell
                        }, null, 8, ["label", "onClick"])
                      ]),
                      default: _withCtx(() => [
                        _createTextVNode(_toDisplayString(_ctx.inShellText), 1)
                      ]),
                      _: 1
                    }))
                  : _createCommentVNode("", true),
                (!_ctx.keysInShell && (_ctx.e2eeNotice || (_ctx.e2eeUnsupported && !_ctx.inThis && _ctx.canJoin)))
                  ? (_openBlock(), _createBlock(_component_q_banner, {
                      key: 3,
                      dense: "",
                      rounded: "",
                      class: "m-tonal text-negative m-call-e2ee-notice"
                    }, {
                      avatar: _withCtx(() => [
                        _createVNode(_component_q_icon, { name: "sym_o_lock" })
                      ]),
                      default: _withCtx(() => [
                        _createTextVNode(_toDisplayString(_ctx.e2eeNotice || _ctx.t('This call is end-to-end encrypted, and this browser cannot encrypt call media (it has no RTCRtpScriptTransform). Join it from a current Chrome, Edge, Firefox or Safari.')), 1)
                      ]),
                      _: 1
                    }))
                  : _createCommentVNode("", true),
                (_ctx.inThis && _ctx.e2ee && _ctx.callKeys && _ctx.callKeys.error)
                  ? (_openBlock(), _createBlock(_component_q_banner, {
                      key: 4,
                      dense: "",
                      rounded: "",
                      class: "m-tonal text-warning"
                    }, {
                      default: _withCtx(() => [
                        _createTextVNode(_toDisplayString(_ctx.callKeys.error), 1)
                      ]),
                      _: 1
                    }))
                  : _createCommentVNode("", true),
                (_ctx.inThis && _ctx.e2ee && _ctx.callKeys && _ctx.callKeys.unverified && _ctx.callKeys.unverified.length)
                  ? (_openBlock(), _createBlock(_component_q_banner, {
                      key: 5,
                      dense: "",
                      rounded: "",
                      class: "m-tonal text-warning m-call-unverified"
                    }, {
                      avatar: _withCtx(() => [
                        _createVNode(_component_q_icon, { name: "sym_o_gpp_maybe" })
                      ]),
                      default: _withCtx(() => [
                        _createTextVNode(_toDisplayString(_ctx.t('Not verified: a device in this call gets its keys though you have not verified it. Verify it in Settings, My devices, if you do not know it:')) + " " + _toDisplayString(_ctx.callKeys.unverified.map((u) => ((_ctx.known && _ctx.known.participants || []).find((p) => p.userId === u.userId) || {}).displayName || u.userId).join(', ')), 1)
                      ]),
                      _: 1
                    }))
                  : _createCommentVNode("", true),
                (_ctx.known && _ctx.known.kind === 'phone')
                  ? (_openBlock(), _createBlock(_component_m_call_context, {
                      key: 6,
                      "call-session-id": _ctx.callSessionId
                    }, null, 8, ["call-session-id"]))
                  : _createCommentVNode("", true),
                (_ctx.recording && _ctx.inThis)
                  ? (_openBlock(), _createBlock(_component_q_banner, {
                      key: 7,
                      dense: "",
                      rounded: "",
                      class: "m-tonal m-call-recording-notice"
                    }, {
                      avatar: _withCtx(() => [
                        _createVNode(_component_q_icon, {
                          name: "sym_o_fiber_manual_record",
                          color: "negative"
                        })
                      ]),
                      action: _withCtx(() => [
                        (_ctx.needsAck || _ctx.withdrawn)
                          ? (_openBlock(), _createBlock(_component_q_btn, {
                              key: 0,
                              flat: "",
                              "no-caps": "",
                              color: "primary",
                              label: _ctx.withdrawn ? _ctx.t('Consent again') : _ctx.t('Acknowledge'),
                              onClick: $event => (_ctx.recAction('ackRecording'))
                            }, null, 8, ["label", "onClick"]))
                          : _createCommentVNode("", true),
                        (!_ctx.withdrawn)
                          ? (_openBlock(), _createBlock(_component_q_btn, {
                              key: 1,
                              flat: "",
                              "no-caps": "",
                              color: "negative",
                              label: _ctx.t('Withdraw consent'),
                              onClick: $event => (_ctx.recAction('withdrawRecording'))
                            }, null, 8, ["label", "onClick"]))
                          : _createCommentVNode("", true)
                      ]),
                      default: _withCtx(() => [
                        _createTextVNode(_toDisplayString(_ctx.withdrawn ? _ctx.t('You withdrew your consent: the recording is paused while you are in the call.') : _ctx.needsAck ? _ctx.t('This call is being recorded. The recording waits until everyone in the call acknowledges.') : _ctx.t('This call is being recorded. Staying in the call is consent.')), 1)
                      ]),
                      _: 1
                    }))
                  : _createCommentVNode("", true),
                (_ctx.rec && _ctx.rec.failed && _ctx.inThis)
                  ? (_openBlock(), _createBlock(_component_q_banner, {
                      key: 8,
                      dense: "",
                      rounded: "",
                      class: "m-tonal text-warning"
                    }, {
                      default: _withCtx(() => [
                        _createTextVNode(_toDisplayString(_ctx.t('The recording stopped: the recorder failed. The call goes on.')), 1)
                      ]),
                      _: 1
                    }))
                  : _createCommentVNode("", true),
                (_openBlock(true), _createElementBlock(_Fragment, null, _renderList((_ctx.inThis || _ctx.isHost) ? _ctx.waiting : [], (p) => {
                  return (_openBlock(), _createBlock(_component_q_banner, {
                    key: 'w' + p.callParticipantId,
                    dense: "",
                    rounded: "",
                    class: "m-tonal"
                  }, {
                    avatar: _withCtx(() => [
                      _createVNode(_component_q_icon, { name: "sym_o_person_add" })
                    ]),
                    action: _withCtx(() => [
                      _createVNode(_component_q_btn, {
                        flat: "",
                        "no-caps": "",
                        color: "positive",
                        label: _ctx.t('Let in'),
                        onClick: $event => (_ctx.admit(p, true))
                      }, null, 8, ["label", "onClick"]),
                      _createVNode(_component_q_btn, {
                        flat: "",
                        "no-caps": "",
                        color: "negative",
                        label: _ctx.t('Turn away'),
                        onClick: $event => (_ctx.admit(p, false))
                      }, null, 8, ["label", "onClick"])
                    ]),
                    default: _withCtx(() => [
                      _createTextVNode(_toDisplayString(p.displayName) + " " + _toDisplayString(_ctx.t('is waiting to join')), 1)
                    ]),
                    _: 2
                  }, 1024))
                }), 128)),
                (!_ctx.known && !_ctx.loading && !_ctx.offline)
                  ? (_openBlock(), _createElementBlock("div", {
                      key: 9,
                      class: "m-muted"
                    }, _toDisplayString(_ctx.t('Call not found')), 1))
                  : _createCommentVNode("", true),
                (_ctx.known && !_ctx.inThis && _ctx.canJoin && !_ctx.keysInShell)
                  ? (_openBlock(), _createElementBlock("div", {
                      key: 10,
                      class: "row q-col-gutter-md"
                    }, [
                      _createElementVNode("div", { class: "col-12 col-md-7" }, [
                        _createElementVNode("div", { style: {"background":"#000","aspect-ratio":"16/9","border-radius":"8px","overflow":"hidden"} }, [
                          (_ctx.preview && _ctx.preview.getVideoTracks().length)
                            ? (_openBlock(), _createBlock(_component_m_call_video, {
                                key: 0,
                                stream: _ctx.preview,
                                muted: "",
                                mirror: "",
                                style: {"width":"100%","height":"100%","object-fit":"cover"}
                              }, null, 8, ["stream"]))
                            : (_openBlock(), _createElementBlock("div", {
                                key: 1,
                                class: "fit flex flex-center text-white"
                              }, _toDisplayString(_ctx.preview ? _ctx.t('Camera off') : _ctx.t('No camera or no permission for it')), 1))
                        ]),
                        _createVNode(_component_q_linear_progress, {
                          value: _ctx.level,
                          class: "q-mt-xs",
                          color: "positive",
                          "aria-label": _ctx.t('Microphone level')
                        }, null, 8, ["value", "aria-label"])
                      ]),
                      _createElementVNode("div", { class: "col-12 col-md-5 column q-gutter-sm" }, [
                        _createVNode(_component_q_select, {
                          dense: "",
                          outlined: "",
                          "emit-value": "",
                          "map-options": "",
                          label: _ctx.t('Microphone'),
                          options: _ctx.audioInputs.map(d => ({value: d.deviceId, label: d.label || _ctx.t('Microphone')})),
                          "model-value": _ctx.call.settings.audioIn,
                          "onUpdate:modelValue": v => _ctx.pick({audioIn: v})
                        }, null, 8, ["label", "options", "model-value", "onUpdate:modelValue"]),
                        _createVNode(_component_q_select, {
                          dense: "",
                          outlined: "",
                          "emit-value": "",
                          "map-options": "",
                          label: _ctx.t('Camera'),
                          options: _ctx.videoInputs.map(d => ({value: d.deviceId, label: d.label || _ctx.t('Camera')})),
                          "model-value": _ctx.call.settings.videoIn,
                          "onUpdate:modelValue": v => _ctx.pick({videoIn: v})
                        }, null, 8, ["label", "options", "model-value", "onUpdate:modelValue"]),
                        _createVNode(_component_q_toggle, {
                          modelValue: _ctx.video,
                          "onUpdate:modelValue": [$event => ((_ctx.video) = $event), $event => (_ctx.pick({}))],
                          label: _ctx.t('Join with video')
                        }, null, 8, ["modelValue", "onUpdate:modelValue", "label"]),
                        _createVNode(_component_q_btn, {
                          unelevated: "",
                          color: "primary",
                          icon: "sym_o_call",
                          disable: _ctx.e2eeUnsupported || _ctx.offline,
                          label: _ctx.known.myLegStatus === 'offered' ? _ctx.t('Answer') : _ctx.t('Join'),
                          onClick: _ctx.join
                        }, null, 8, ["disable", "label", "onClick"])
                      ])
                    ]))
                  : _createCommentVNode("", true),
                (_ctx.inThis && _ctx.mainTile)
                  ? (_openBlock(), _createElementBlock("div", {
                      key: 11,
                      style: {"height":"60vh"},
                      class: "q-mb-sm"
                    }, [
                      (_openBlock(true), _createElementBlock(_Fragment, null, _renderList([_ctx.mainTile], (tile) => {
                        return (_openBlock(), _createElementBlock("div", {
                          key: 'main' + tile.key,
                          class: "fit"
                        }, [
                          _createElementVNode("div", {
                            class: "m-call-tile",
                            style: _normalizeStyle(_ctx.tileStyle(tile))
                          }, [
                            (tile.stream)
                              ? (_openBlock(), _createBlock(_component_m_call_video, {
                                  key: 0,
                                  stream: tile.stream,
                                  muted: "",
                                  mirror: !!tile.mirror,
                                  style: _normalizeStyle(tile.screen ? 'width:100%;height:100%;object-fit:contain' : 'width:100%;height:100%;object-fit:cover')
                                }, null, 8, ["stream", "mirror", "style"]))
                              : (_openBlock(), _createElementBlock("div", {
                                  key: 1,
                                  class: "fit flex flex-center text-white text-subtitle1"
                                }, [
                                  (tile.phone)
                                    ? (_openBlock(), _createBlock(_component_q_icon, {
                                        key: 0,
                                        name: "sym_o_call",
                                        class: "q-mr-xs"
                                      }))
                                    : _createCommentVNode("", true),
                                  _createTextVNode(_toDisplayString(tile.name), 1)
                                ])),
                            (tile.reaction)
                              ? (_openBlock(), _createElementBlock("div", {
                                  key: 2,
                                  class: "absolute-center text-h3"
                                }, _toDisplayString(tile.reaction), 1))
                              : _createCommentVNode("", true),
                            (tile.audioOnly)
                              ? (_openBlock(), _createBlock(_component_q_chip, {
                                  key: 3,
                                  dense: "",
                                  square: "",
                                  class: "absolute-top-right q-ma-xs",
                                  color: "grey-9",
                                  "text-color": "white",
                                  icon: "sym_o_signal_cellular_alt_1_bar"
                                }, {
                                  default: _withCtx(() => [
                                    _createTextVNode(_toDisplayString(_ctx.t('Audio only: low bandwidth')), 1)
                                  ]),
                                  _: 1
                                }))
                              : _createCommentVNode("", true),
                            _createElementVNode("div", { class: "absolute-bottom-left q-pa-xs text-white text-caption" }, [
                              _createTextVNode(_toDisplayString(tile.name), 1),
                              (tile.muted)
                                ? (_openBlock(), _createBlock(_component_q_icon, {
                                    key: 0,
                                    name: "sym_o_mic_off",
                                    class: "q-ml-xs"
                                  }))
                                : _createCommentVNode("", true),
                              (tile.hand)
                                ? (_openBlock(), _createBlock(_component_q_icon, {
                                    key: 1,
                                    name: "sym_o_back_hand",
                                    color: "warning",
                                    class: "q-ml-xs"
                                  }))
                                : _createCommentVNode("", true)
                            ])
                          ], 4)
                        ]))
                      }), 128))
                    ]))
                  : _createCommentVNode("", true),
                (_ctx.inThis)
                  ? (_openBlock(), _createElementBlock("div", {
                      key: 12,
                      style: _normalizeStyle(_ctx.gridStyle)
                    }, [
                      (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.stripTiles, (tile) => {
                        return (_openBlock(), _createElementBlock("div", {
                          key: tile.key
                        }, [
                          _createElementVNode("div", {
                            class: "m-call-tile",
                            style: _normalizeStyle(_ctx.tileStyle(tile))
                          }, [
                            (tile.stream)
                              ? (_openBlock(), _createBlock(_component_m_call_video, {
                                  key: 0,
                                  stream: tile.stream,
                                  muted: "",
                                  mirror: !!tile.mirror,
                                  style: _normalizeStyle(tile.screen ? 'width:100%;height:100%;object-fit:contain' : 'width:100%;height:100%;object-fit:cover')
                                }, null, 8, ["stream", "mirror", "style"]))
                              : (_openBlock(), _createElementBlock("div", {
                                  key: 1,
                                  class: "fit flex flex-center text-white text-subtitle1"
                                }, [
                                  (tile.phone)
                                    ? (_openBlock(), _createBlock(_component_q_icon, {
                                        key: 0,
                                        name: "sym_o_call",
                                        class: "q-mr-xs"
                                      }))
                                    : _createCommentVNode("", true),
                                  _createTextVNode(_toDisplayString(tile.name), 1)
                                ])),
                            (tile.reaction)
                              ? (_openBlock(), _createElementBlock("div", {
                                  key: 2,
                                  class: "absolute-center text-h3"
                                }, _toDisplayString(tile.reaction), 1))
                              : _createCommentVNode("", true),
                            (tile.audioOnly)
                              ? (_openBlock(), _createBlock(_component_q_chip, {
                                  key: 3,
                                  dense: "",
                                  square: "",
                                  class: "absolute-top-right q-ma-xs",
                                  color: "grey-9",
                                  "text-color": "white",
                                  icon: "sym_o_signal_cellular_alt_1_bar"
                                }, {
                                  default: _withCtx(() => [
                                    _createTextVNode(_toDisplayString(_ctx.t('Audio only: low bandwidth')), 1)
                                  ]),
                                  _: 1
                                }))
                              : _createCommentVNode("", true),
                            _createElementVNode("div", { class: "absolute-bottom-left q-pa-xs text-white text-caption" }, [
                              _createTextVNode(_toDisplayString(tile.name), 1),
                              (tile.muted)
                                ? (_openBlock(), _createBlock(_component_q_icon, {
                                    key: 0,
                                    name: "sym_o_mic_off",
                                    class: "q-ml-xs"
                                  }))
                                : _createCommentVNode("", true),
                              (tile.hand)
                                ? (_openBlock(), _createBlock(_component_q_icon, {
                                    key: 1,
                                    name: "sym_o_back_hand",
                                    color: "warning",
                                    class: "q-ml-xs"
                                  }))
                                : _createCommentVNode("", true)
                            ])
                          ], 4)
                        ]))
                      }), 128))
                    ], 4))
                  : _createCommentVNode("", true),
                (_ctx.inThis && !_ctx.remotes.length)
                  ? (_openBlock(), _createElementBlock("div", {
                      key: 13,
                      class: "m-muted"
                    }, _toDisplayString(_ctx.call.phase === 'connected' ? _ctx.t('Waiting for the others') : _ctx.t('Connecting')), 1))
                  : _createCommentVNode("", true),
                (_ctx.inThis && (_ctx.call.captions.on || _ctx.call.captions.unavailable))
                  ? (_openBlock(), _createElementBlock("div", {
                      key: 14,
                      class: "m-call-captions q-pa-sm rounded-borders",
                      style: {"background":"rgba(0,0,0,.78)","color":"#fff"},
                      "aria-live": "polite"
                    }, [
                      _createElementVNode("div", {
                        class: "text-caption",
                        style: {"opacity":".85"}
                      }, [
                        _createVNode(_component_q_icon, {
                          name: "sym_o_closed_caption",
                          class: "q-mr-xs"
                        }),
                        _createTextVNode(_toDisplayString(_ctx.captionNotice), 1)
                      ]),
                      (_ctx.call.captions.unavailable)
                        ? (_openBlock(), _createElementBlock("div", {
                            key: 0,
                            class: "text-caption text-warning"
                          }, [
                            _createVNode(_component_q_icon, {
                              name: "sym_o_warning",
                              class: "q-mr-xs"
                            }),
                            _createTextVNode(_toDisplayString(_ctx.t('Captions are unavailable')) + ": " + _toDisplayString(_ctx.call.captions.unavailable), 1)
                          ]))
                        : _createCommentVNode("", true),
                      (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.captionLines, (line) => {
                        return (_openBlock(), _createElementBlock("div", {
                          key: line.key,
                          class: "text-body2"
                        }, [
                          _createElementVNode("span", { class: "text-weight-bold" }, _toDisplayString(line.name) + ":", 1),
                          _createTextVNode(),
                          _createElementVNode("span", {
                            style: _normalizeStyle(line.gap ? 'opacity:.6' : '')
                          }, _toDisplayString(line.text), 5)
                        ]))
                      }), 128))
                    ]))
                  : _createCommentVNode("", true),
                (_ctx.inThis)
                  ? (_openBlock(), _createElementBlock("div", {
                      key: 15,
                      class: "row justify-center q-gutter-sm q-mt-sm"
                    }, [
                      _createVNode(_component_q_btn, {
                        round: "",
                        unelevated: "",
                        color: _ctx.call.muted ? 'negative' : 'grey-8',
                        icon: _ctx.call.muted ? 'sym_o_mic_off' : 'sym_o_mic',
                        "aria-label": _ctx.t('Mute'),
                        onClick: $event => (_ctx.moquiCall().toggleMute())
                      }, {
                        default: _withCtx(() => [
                          _createVNode(_component_q_tooltip, null, {
                            default: _withCtx(() => [
                              _createTextVNode(_toDisplayString(_ctx.call.muted ? _ctx.t('Unmute') : _ctx.t('Mute')), 1)
                            ]),
                            _: 1
                          })
                        ]),
                        _: 1
                      }, 8, ["color", "icon", "aria-label", "onClick"]),
                      _createVNode(_component_q_btn, {
                        round: "",
                        unelevated: "",
                        color: _ctx.call.cameraOff ? 'negative' : 'grey-8',
                        icon: _ctx.call.cameraOff ? 'sym_o_videocam_off' : 'sym_o_videocam',
                        "aria-label": _ctx.t('Camera'),
                        disable: !_ctx.call.localStream || !_ctx.call.localStream.getVideoTracks().length,
                        onClick: $event => (_ctx.moquiCall().toggleCamera())
                      }, {
                        default: _withCtx(() => [
                          _createVNode(_component_q_tooltip, null, {
                            default: _withCtx(() => [
                              _createTextVNode(_toDisplayString(_ctx.t('Camera')), 1)
                            ]),
                            _: 1
                          })
                        ]),
                        _: 1
                      }, 8, ["color", "icon", "aria-label", "disable", "onClick"]),
                      (_ctx.canShare)
                        ? (_openBlock(), _createBlock(_component_q_btn, {
                            key: 0,
                            round: "",
                            unelevated: "",
                            color: _ctx.call.sharing ? 'primary' : 'grey-8',
                            icon: _ctx.call.sharing ? 'sym_o_stop_screen_share' : 'sym_o_present_to_all',
                            "aria-label": _ctx.call.sharing ? _ctx.t('Stop sharing') : _ctx.t('Share the screen'),
                            onClick: $event => (_ctx.call.sharing ? _ctx.moquiCall().stopShare() : _ctx.moquiCall().shareScreen())
                          }, {
                            default: _withCtx(() => [
                              _createVNode(_component_q_tooltip, null, {
                                default: _withCtx(() => [
                                  _createTextVNode(_toDisplayString(_ctx.call.sharing ? _ctx.t('Stop sharing') : _ctx.t('Share the screen')), 1)
                                ]),
                                _: 1
                              })
                            ]),
                            _: 1
                          }, 8, ["color", "icon", "aria-label", "onClick"]))
                        : _createCommentVNode("", true),
                      _createVNode(_component_q_btn, {
                        round: "",
                        unelevated: "",
                        color: _ctx.call.hand ? 'warning' : 'grey-8',
                        icon: "sym_o_back_hand",
                        "aria-label": _ctx.t('Raise your hand'),
                        onClick: $event => (_ctx.moquiCall().toggleHand())
                      }, {
                        default: _withCtx(() => [
                          _createVNode(_component_q_tooltip, null, {
                            default: _withCtx(() => [
                              _createTextVNode(_toDisplayString(_ctx.call.hand ? _ctx.t('Lower your hand') : _ctx.t('Raise your hand')), 1)
                            ]),
                            _: 1
                          })
                        ]),
                        _: 1
                      }, 8, ["color", "aria-label", "onClick"]),
                      (_ctx.call.captions.allowed || _ctx.call.captions.on)
                        ? (_openBlock(), _createBlock(_component_q_btn, {
                            key: 1,
                            round: "",
                            unelevated: "",
                            color: _ctx.call.captions.on ? 'primary' : 'grey-8',
                            icon: "sym_o_closed_caption",
                            "aria-label": _ctx.call.captions.on ? _ctx.t('Turn off captions') : _ctx.t('Turn on captions'),
                            onClick: $event => (_ctx.moquiCall().toggleCaptions())
                          }, {
                            default: _withCtx(() => [
                              _createVNode(_component_q_tooltip, null, {
                                default: _withCtx(() => [
                                  _createTextVNode(_toDisplayString(_ctx.call.captions.on ? _ctx.t('Turn off captions') : _ctx.t('Turn on captions')), 1)
                                ]),
                                _: 1
                              })
                            ]),
                            _: 1
                          }, 8, ["color", "aria-label", "onClick"]))
                        : _createCommentVNode("", true),
                      _createVNode(_component_q_btn, {
                        round: "",
                        unelevated: "",
                        color: "grey-8",
                        icon: "sym_o_add_reaction",
                        "aria-label": _ctx.t('React')
                      }, {
                        default: _withCtx(() => [
                          _createVNode(_component_q_menu, null, {
                            default: _withCtx(() => [
                              _createElementVNode("div", { class: "row q-pa-xs" }, [
                                (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.reactions, (emoji) => {
                                  return _withDirectives((_openBlock(), _createBlock(_component_q_btn, {
                                    key: emoji,
                                    flat: "",
                                    dense: "",
                                    size: "lg",
                                    onClick: $event => (_ctx.moquiCall().sendReaction(emoji))
                                  }, {
                                    default: _withCtx(() => [
                                      _createTextVNode(_toDisplayString(emoji), 1)
                                    ]),
                                    _: 2
                                  }, 1032, ["onClick"])), [
                                    [_directive_close_popup]
                                  ])
                                }), 128))
                              ])
                            ]),
                            _: 1
                          })
                        ]),
                        _: 1
                      }, 8, ["aria-label"]),
                      _createVNode(_component_q_btn, {
                        round: "",
                        unelevated: "",
                        color: "grey-8",
                        icon: _ctx.call.layout === 'speaker' ? 'sym_o_grid_view' : 'sym_o_person',
                        "aria-label": _ctx.call.layout === 'speaker' ? _ctx.t('Grid view') : _ctx.t('Speaker view'),
                        onClick: $event => (_ctx.moquiCall().setLayout(_ctx.call.layout === 'speaker' ? 'grid' : 'speaker'))
                      }, {
                        default: _withCtx(() => [
                          _createVNode(_component_q_tooltip, null, {
                            default: _withCtx(() => [
                              _createTextVNode(_toDisplayString(_ctx.call.layout === 'speaker' ? _ctx.t('Grid view') : _ctx.t('Speaker view')), 1)
                            ]),
                            _: 1
                          })
                        ]),
                        _: 1
                      }, 8, ["icon", "aria-label", "onClick"]),
                      _createVNode(_component_q_btn, {
                        round: "",
                        unelevated: "",
                        color: "grey-8",
                        icon: "sym_o_group",
                        "aria-label": _ctx.t('Participants'),
                        onClick: $event => (_ctx.call.participantsOpen = !_ctx.call.participantsOpen)
                      }, {
                        default: _withCtx(() => [
                          (_ctx.raisedCount)
                            ? (_openBlock(), _createBlock(_component_q_badge, {
                                key: 0,
                                color: "warning",
                                floating: ""
                              }, {
                                default: _withCtx(() => [
                                  _createTextVNode(_toDisplayString(_ctx.raisedCount), 1)
                                ]),
                                _: 1
                              }))
                            : _createCommentVNode("", true),
                          _createVNode(_component_q_tooltip, null, {
                            default: _withCtx(() => [
                              _createTextVNode(_toDisplayString(_ctx.t('Participants')), 1)
                            ]),
                            _: 1
                          })
                        ]),
                        _: 1
                      }, 8, ["aria-label", "onClick"]),
                      (_ctx.canRecord && !_ctx.recording)
                        ? (_openBlock(), _createBlock(_component_q_btn, {
                            key: 2,
                            round: "",
                            unelevated: "",
                            color: "grey-8",
                            icon: "sym_o_radio_button_checked",
                            "aria-label": _ctx.t('Record'),
                            onClick: $event => (_ctx.recAction('startRecording'))
                          }, {
                            default: _withCtx(() => [
                              _createVNode(_component_q_tooltip, null, {
                                default: _withCtx(() => [
                                  _createTextVNode(_toDisplayString(_ctx.t('Record the call')), 1)
                                ]),
                                _: 1
                              })
                            ]),
                            _: 1
                          }, 8, ["aria-label", "onClick"]))
                        : _createCommentVNode("", true),
                      (_ctx.canRecord && _ctx.recording)
                        ? (_openBlock(), _createBlock(_component_q_btn, {
                            key: 3,
                            round: "",
                            unelevated: "",
                            color: "negative",
                            icon: "sym_o_radio_button_checked",
                            "aria-label": _ctx.t('Recording')
                          }, {
                            default: _withCtx(() => [
                              _createVNode(_component_q_tooltip, null, {
                                default: _withCtx(() => [
                                  _createTextVNode(_toDisplayString(_ctx.recordingText), 1)
                                ]),
                                _: 1
                              }),
                              _createVNode(_component_q_menu, null, {
                                default: _withCtx(() => [
                                  _createElementVNode("div", { class: "column q-pa-xs" }, [
                                    (_ctx.recording.state === 'recording')
                                      ? _withDirectives((_openBlock(), _createBlock(_component_q_btn, {
                                          key: 0,
                                          flat: "",
                                          "no-caps": "",
                                          align: "left",
                                          icon: "sym_o_pause",
                                          label: _ctx.t('Pause recording'),
                                          onClick: $event => (_ctx.recAction('pauseRecording'))
                                        }, null, 8, ["label", "onClick"])), [
                                          [_directive_close_popup]
                                        ])
                                      : _createCommentVNode("", true),
                                    (_ctx.recording.state === 'paused' && _ctx.recording.pauseReason === 'user')
                                      ? _withDirectives((_openBlock(), _createBlock(_component_q_btn, {
                                          key: 1,
                                          flat: "",
                                          "no-caps": "",
                                          align: "left",
                                          icon: "sym_o_play_arrow",
                                          label: _ctx.t('Resume recording'),
                                          onClick: $event => (_ctx.recAction('resumeRecording'))
                                        }, null, 8, ["label", "onClick"])), [
                                          [_directive_close_popup]
                                        ])
                                      : _createCommentVNode("", true),
                                    _withDirectives(_createVNode(_component_q_btn, {
                                      flat: "",
                                      "no-caps": "",
                                      align: "left",
                                      color: "negative",
                                      icon: "sym_o_stop",
                                      label: _ctx.t('Stop recording'),
                                      onClick: $event => (_ctx.recAction('stopRecording'))
                                    }, null, 8, ["label", "onClick"]), [
                                      [_directive_close_popup]
                                    ])
                                  ])
                                ]),
                                _: 1
                              })
                            ]),
                            _: 1
                          }, 8, ["aria-label"]))
                        : _createCommentVNode("", true),
                      (_ctx.hasPhone)
                        ? (_openBlock(), _createBlock(_component_q_btn, {
                            key: 4,
                            round: "",
                            unelevated: "",
                            color: "grey-8",
                            icon: "sym_o_dialpad",
                            "aria-label": _ctx.t('Keypad')
                          }, {
                            default: _withCtx(() => [
                              _createVNode(_component_q_tooltip, null, {
                                default: _withCtx(() => [
                                  _createTextVNode(_toDisplayString(_ctx.t('Keypad')), 1)
                                ]),
                                _: 1
                              }),
                              _createVNode(_component_q_menu, null, {
                                default: _withCtx(() => [
                                  _createElementVNode("div", {
                                    class: "q-pa-sm",
                                    style: {"display":"grid","grid-template-columns":"repeat(3,48px)","gap":"4px"}
                                  }, [
                                    (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.keys, (key) => {
                                      return (_openBlock(), _createBlock(_component_q_btn, {
                                        key: key,
                                        outline: "",
                                        dense: "",
                                        size: "lg",
                                        onClick: $event => (_ctx.dtmf(key))
                                      }, {
                                        default: _withCtx(() => [
                                          _createTextVNode(_toDisplayString(key), 1)
                                        ]),
                                        _: 2
                                      }, 1032, ["onClick"]))
                                    }), 128))
                                  ])
                                ]),
                                _: 1
                              })
                            ]),
                            _: 1
                          }, 8, ["aria-label"]))
                        : _createCommentVNode("", true),
                      (_ctx.inThis && _ctx.known && _ctx.known.status !== 'ended')
                        ? (_openBlock(), _createBlock(_component_m_call_actions, {
                            key: 5,
                            view: _ctx.known
                          }, null, 8, ["view"]))
                        : _createCommentVNode("", true),
                      _createVNode(_component_q_btn, {
                        round: "",
                        unelevated: "",
                        color: "negative",
                        icon: "sym_o_call_end",
                        "aria-label": _ctx.t('Leave'),
                        onClick: _ctx.leave
                      }, {
                        default: _withCtx(() => [
                          _createVNode(_component_q_tooltip, null, {
                            default: _withCtx(() => [
                              _createTextVNode(_toDisplayString(_ctx.t('Leave')), 1)
                            ]),
                            _: 1
                          })
                        ]),
                        _: 1
                      }, 8, ["aria-label", "onClick"]),
                      (_ctx.isHost && _ctx.known && _ctx.known.kind !== 'direct')
                        ? (_openBlock(), _createBlock(_component_q_btn, {
                            key: 6,
                            flat: "",
                            "no-caps": "",
                            color: "negative",
                            label: _ctx.t('End for everyone'),
                            onClick: _ctx.endForAll
                          }, null, 8, ["label", "onClick"]))
                        : _createCommentVNode("", true),
                      (_ctx.known)
                        ? (_openBlock(), _createBlock(_component_m_ext_actions, {
                            key: 7,
                            slot: "call-actions",
                            item: {callSessionId: _ctx.callSessionId}
                          }, null, 8, ["item"]))
                        : _createCommentVNode("", true)
                    ]))
                  : _createCommentVNode("", true),
                (_ctx.inThis && _ctx.call.participantsOpen && _ctx.known)
                  ? (_openBlock(), _createBlock(_component_q_list, {
                      key: 16,
                      bordered: "",
                      separator: "",
                      dense: "",
                      class: "rounded-borders q-mt-sm"
                    }, {
                      default: _withCtx(() => [
                        (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.known.participants || [], (p) => {
                          return (_openBlock(), _createBlock(_component_q_item, {
                            key: p.callParticipantId
                          }, {
                            default: _withCtx(() => [
                              _createVNode(_component_q_item_section, { avatar: "" }, {
                                default: _withCtx(() => [
                                  _createVNode(_component_m_avatar, {
                                    name: p.displayName,
                                    "user-id": p.userId || undefined,
                                    size: "28px"
                                  }, null, 8, ["name", "user-id"])
                                ]),
                                _: 2
                              }, 1024),
                              _createVNode(_component_q_item_section, null, {
                                default: _withCtx(() => [
                                  _createVNode(_component_q_item_label, null, {
                                    default: _withCtx(() => [
                                      _createTextVNode(_toDisplayString(p.displayName), 1),
                                      (p.role === 'host')
                                        ? (_openBlock(), _createElementBlock("span", {
                                            key: 0,
                                            class: "text-caption m-muted"
                                          }, " · " + _toDisplayString(_ctx.t('host')), 1))
                                        : _createCommentVNode("", true)
                                    ]),
                                    _: 2
                                  }, 1024),
                                  _createVNode(_component_q_item_label, { caption: "" }, {
                                    default: _withCtx(() => [
                                      _createTextVNode(_toDisplayString(p.inCall ? _ctx.t('In the call') : p.ringing ? _ctx.t('Ringing') : _ctx.t('Not in the call')), 1),
                                      (_ctx.keyText(p))
                                        ? (_openBlock(), _createElementBlock("span", {
                                            key: 0,
                                            class: "m-call-key-state"
                                          }, [
                                            _createTextVNode(" · "),
                                            _createVNode(_component_q_icon, {
                                              name: "sym_o_key",
                                              size: "14px"
                                            }),
                                            _createTextVNode(" " + _toDisplayString(_ctx.keyText(p)), 1)
                                          ]))
                                        : _createCommentVNode("", true)
                                    ]),
                                    _: 2
                                  }, 1024)
                                ]),
                                _: 2
                              }, 1024),
                              _createVNode(_component_q_item_section, {
                                side: "",
                                class: "row no-wrap items-center"
                              }, {
                                default: _withCtx(() => [
                                  (_ctx.handOf(p))
                                    ? (_openBlock(), _createBlock(_component_q_icon, {
                                        key: 0,
                                        name: "sym_o_back_hand",
                                        color: "warning"
                                      }))
                                    : _createCommentVNode("", true),
                                  (_ctx.isHost && p.userId !== _ctx.moquiCallMe() && p.admission === 'admitted')
                                    ? (_openBlock(), _createBlock(_component_q_btn, {
                                        key: 1,
                                        flat: "",
                                        dense: "",
                                        round: "",
                                        size: "sm",
                                        icon: "sym_o_person_remove",
                                        "aria-label": _ctx.t('Remove from the call'),
                                        onClick: $event => (_ctx.remove(p))
                                      }, {
                                        default: _withCtx(() => [
                                          _createVNode(_component_q_tooltip, null, {
                                            default: _withCtx(() => [
                                              _createTextVNode(_toDisplayString(_ctx.t('Remove from the call')), 1)
                                            ]),
                                            _: 1
                                          })
                                        ]),
                                        _: 1
                                      }, 8, ["aria-label", "onClick"]))
                                    : _createCommentVNode("", true)
                                ]),
                                _: 2
                              }, 1024)
                            ]),
                            _: 2
                          }, 1024))
                        }), 128))
                      ]),
                      _: 1
                    }))
                  : _createCommentVNode("", true)
              ]))
            }
        } },
        "m-call-actions": { hash: "5200a217", make: function (Vue) {
            const { toDisplayString: _toDisplayString, createTextVNode: _createTextVNode, resolveComponent: _resolveComponent, withCtx: _withCtx, createVNode: _createVNode, openBlock: _openBlock, createBlock: _createBlock, createCommentVNode: _createCommentVNode, resolveDirective: _resolveDirective, withDirectives: _withDirectives, createElementVNode: _createElementVNode, createElementBlock: _createElementBlock } = Vue

            return function render(_ctx, _cache) {
              const _component_q_tooltip = _resolveComponent("q-tooltip")
              const _component_q_btn = _resolveComponent("q-btn")
              const _component_q_icon = _resolveComponent("q-icon")
              const _component_q_item_section = _resolveComponent("q-item-section")
              const _component_q_item = _resolveComponent("q-item")
              const _component_q_list = _resolveComponent("q-list")
              const _component_q_menu = _resolveComponent("q-menu")
              const _component_q_banner = _resolveComponent("q-banner")
              const _component_q_card_section = _resolveComponent("q-card-section")
              const _component_q_select = _resolveComponent("q-select")
              const _component_q_btn_toggle = _resolveComponent("q-btn-toggle")
              const _component_q_input = _resolveComponent("q-input")
              const _component_q_option_group = _resolveComponent("q-option-group")
              const _component_q_card_actions = _resolveComponent("q-card-actions")
              const _component_q_card = _resolveComponent("q-card")
              const _component_q_dialog = _resolveComponent("q-dialog")
              const _directive_close_popup = _resolveDirective("close-popup")

              return (_openBlock(), _createElementBlock("span", { class: "m-call-actions" }, [
                (_ctx.hasPhone)
                  ? (_openBlock(), _createBlock(_component_q_btn, {
                      key: 0,
                      round: "",
                      unelevated: "",
                      color: _ctx.held ? 'warning' : 'grey-8',
                      icon: _ctx.held ? 'sym_o_play_arrow' : 'sym_o_pause',
                      disable: _ctx.busy,
                      "aria-label": _ctx.held ? _ctx.t('Resume') : _ctx.t('Hold'),
                      onClick: _ctx.hold,
                      class: "q-ml-xs"
                    }, {
                      default: _withCtx(() => [
                        _createVNode(_component_q_tooltip, null, {
                          default: _withCtx(() => [
                            _createTextVNode(_toDisplayString(_ctx.held ? _ctx.t('Take off hold') : _ctx.t('Put on hold')), 1)
                          ]),
                          _: 1
                        })
                      ]),
                      _: 1
                    }, 8, ["color", "icon", "disable", "aria-label", "onClick"]))
                  : _createCommentVNode("", true),
                _createVNode(_component_q_btn, {
                  round: "",
                  unelevated: "",
                  color: "grey-8",
                  icon: "sym_o_more_horiz",
                  "aria-label": _ctx.t('More call actions'),
                  class: "q-ml-xs"
                }, {
                  default: _withCtx(() => [
                    _createVNode(_component_q_menu, null, {
                      default: _withCtx(() => [
                        _createVNode(_component_q_list, {
                          dense: "",
                          style: {"min-width":"220px"}
                        }, {
                          default: _withCtx(() => [
                            (_ctx.hasPhone && !_ctx.view.transfer)
                              ? _withDirectives((_openBlock(), _createBlock(_component_q_item, {
                                  key: 0,
                                  clickable: "",
                                  onClick: $event => (_ctx.open('transfer'))
                                }, {
                                  default: _withCtx(() => [
                                    _createVNode(_component_q_item_section, { avatar: "" }, {
                                      default: _withCtx(() => [
                                        _createVNode(_component_q_icon, { name: "sym_o_phone_forwarded" })
                                      ]),
                                      _: 1
                                    }),
                                    _createVNode(_component_q_item_section, null, {
                                      default: _withCtx(() => [
                                        _createTextVNode(_toDisplayString(_ctx.t('Transfer')), 1)
                                      ]),
                                      _: 1
                                    })
                                  ]),
                                  _: 1
                                }, 8, ["onClick"])), [
                                  [_directive_close_popup]
                                ])
                              : _createCommentVNode("", true),
                            _withDirectives((_openBlock(), _createBlock(_component_q_item, {
                              clickable: "",
                              onClick: $event => (_ctx.open('add'))
                            }, {
                              default: _withCtx(() => [
                                _createVNode(_component_q_item_section, { avatar: "" }, {
                                  default: _withCtx(() => [
                                    _createVNode(_component_q_icon, { name: "sym_o_person_add" })
                                  ]),
                                  _: 1
                                }),
                                _createVNode(_component_q_item_section, null, {
                                  default: _withCtx(() => [
                                    _createTextVNode(_toDisplayString(_ctx.t('Add a participant')), 1)
                                  ]),
                                  _: 1
                                })
                              ]),
                              _: 1
                            }, 8, ["onClick"])), [
                              [_directive_close_popup]
                            ]),
                            (_ctx.otherCalls.length && !_ctx.view.e2ee)
                              ? _withDirectives((_openBlock(), _createBlock(_component_q_item, {
                                  key: 1,
                                  clickable: "",
                                  onClick: $event => (_ctx.open('merge'))
                                }, {
                                  default: _withCtx(() => [
                                    _createVNode(_component_q_item_section, { avatar: "" }, {
                                      default: _withCtx(() => [
                                        _createVNode(_component_q_icon, { name: "sym_o_call_merge" })
                                      ]),
                                      _: 1
                                    }),
                                    _createVNode(_component_q_item_section, null, {
                                      default: _withCtx(() => [
                                        _createTextVNode(_toDisplayString(_ctx.t('Merge another call')), 1)
                                      ]),
                                      _: 1
                                    })
                                  ]),
                                  _: 1
                                }, 8, ["onClick"])), [
                                  [_directive_close_popup]
                                ])
                              : _createCommentVNode("", true),
                            (_ctx.view.kind !== 'meeting')
                              ? _withDirectives((_openBlock(), _createBlock(_component_q_item, {
                                  key: 2,
                                  clickable: "",
                                  onClick: _ctx.meeting
                                }, {
                                  default: _withCtx(() => [
                                    _createVNode(_component_q_item_section, { avatar: "" }, {
                                      default: _withCtx(() => [
                                        _createVNode(_component_q_icon, { name: "sym_o_groups" })
                                      ]),
                                      _: 1
                                    }),
                                    _createVNode(_component_q_item_section, null, {
                                      default: _withCtx(() => [
                                        _createTextVNode(_toDisplayString(_ctx.t('Make it a meeting')), 1)
                                      ]),
                                      _: 1
                                    })
                                  ]),
                                  _: 1
                                }, 8, ["onClick"])), [
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
                (_ctx.myConsult)
                  ? (_openBlock(), _createBlock(_component_q_banner, {
                      key: 1,
                      dense: "",
                      rounded: "",
                      class: "m-tonal q-mt-sm"
                    }, {
                      avatar: _withCtx(() => [
                        _createVNode(_component_q_icon, { name: "sym_o_phone_forwarded" })
                      ]),
                      action: _withCtx(() => [
                        _createVNode(_component_q_btn, {
                          flat: "",
                          "no-caps": "",
                          color: "positive",
                          label: _ctx.t('Complete transfer'),
                          onClick: $event => (_ctx.finish(true))
                        }, null, 8, ["label", "onClick"]),
                        _createVNode(_component_q_btn, {
                          flat: "",
                          "no-caps": "",
                          color: "negative",
                          label: _ctx.t('Cancel transfer'),
                          onClick: $event => (_ctx.finish(false))
                        }, null, 8, ["label", "onClick"])
                      ]),
                      default: _withCtx(() => [
                        _createTextVNode(_toDisplayString(_ctx.t('Transferring to')) + " " + _toDisplayString(_ctx.myConsult.targetName) + ": " + _toDisplayString(_ctx.t('the caller is on hold')), 1)
                      ]),
                      _: 1
                    }))
                  : _createCommentVNode("", true),
                _createVNode(_component_q_dialog, {
                  "model-value": !!_ctx.dialog,
                  "onUpdate:modelValue": v => { if (!v) _ctx.dialog = null }
                }, {
                  default: _withCtx(() => [
                    _createVNode(_component_q_card, { style: {"min-width":"360px"} }, {
                      default: _withCtx(() => [
                        _createVNode(_component_q_card_section, { class: "text-h6" }, {
                          default: _withCtx(() => [
                            _createTextVNode(_toDisplayString(_ctx.dialog === 'transfer' ? _ctx.t('Transfer the call') : _ctx.dialog === 'add' ? _ctx.t('Add a participant') : _ctx.t('Merge another call into this one')), 1)
                          ]),
                          _: 1
                        }),
                        (_ctx.dialog === 'merge')
                          ? (_openBlock(), _createBlock(_component_q_card_section, { key: 0 }, {
                              default: _withCtx(() => [
                                _createVNode(_component_q_select, {
                                  dense: "",
                                  outlined: "",
                                  "emit-value": "",
                                  "map-options": "",
                                  modelValue: _ctx.otherId,
                                  "onUpdate:modelValue": $event => ((_ctx.otherId) = $event),
                                  options: _ctx.otherCalls,
                                  label: _ctx.t('Call')
                                }, null, 8, ["modelValue", "onUpdate:modelValue", "options", "label"]),
                                _createElementVNode("div", { class: "text-caption m-muted q-mt-sm" }, _toDisplayString(_ctx.t('The calls become one meeting.')), 1)
                              ]),
                              _: 1
                            }))
                          : (_openBlock(), _createBlock(_component_q_card_section, {
                              key: 1,
                              class: "q-gutter-sm"
                            }, {
                              default: _withCtx(() => [
                                _createVNode(_component_q_btn_toggle, {
                                  modelValue: _ctx.targetType,
                                  "onUpdate:modelValue": $event => ((_ctx.targetType) = $event),
                                  "no-caps": "",
                                  dense: "",
                                  unelevated: "",
                                  "toggle-color": "primary",
                                  options: _ctx.dialog === 'transfer' ? [{label: _ctx.t('Person'), value: 'user'}, {label: _ctx.t('Number'), value: 'number'}, {label: _ctx.t('Queue'), value: 'queue'}] : _ctx.view.e2ee ? [{label: _ctx.t('Person'), value: 'user'}] : [{label: _ctx.t('Person'), value: 'user'}, {label: _ctx.t('Number'), value: 'number'}]
                                }, null, 8, ["modelValue", "onUpdate:modelValue", "options"]),
                                (_ctx.targetType === 'user')
                                  ? (_openBlock(), _createBlock(_component_q_select, {
                                      key: 0,
                                      dense: "",
                                      outlined: "",
                                      "use-input": "",
                                      "emit-value": "",
                                      "map-options": "",
                                      modelValue: _ctx.userId,
                                      "onUpdate:modelValue": $event => ((_ctx.userId) = $event),
                                      options: _ctx.userOptions,
                                      onFilter: _ctx.searchUsers,
                                      label: _ctx.t('Who')
                                    }, null, 8, ["modelValue", "onUpdate:modelValue", "options", "onFilter", "label"]))
                                  : _createCommentVNode("", true),
                                (_ctx.targetType === 'number')
                                  ? (_openBlock(), _createBlock(_component_q_input, {
                                      key: 1,
                                      dense: "",
                                      outlined: "",
                                      modelValue: _ctx.number,
                                      "onUpdate:modelValue": $event => ((_ctx.number) = $event),
                                      label: _ctx.t('Number'),
                                      type: "tel"
                                    }, null, 8, ["modelValue", "onUpdate:modelValue", "label"]))
                                  : _createCommentVNode("", true),
                                (_ctx.targetType === 'queue')
                                  ? (_openBlock(), _createBlock(_component_q_select, {
                                      key: 2,
                                      dense: "",
                                      outlined: "",
                                      "emit-value": "",
                                      "map-options": "",
                                      modelValue: _ctx.queueId,
                                      "onUpdate:modelValue": $event => ((_ctx.queueId) = $event),
                                      options: _ctx.queueOptions,
                                      label: _ctx.t('Queue')
                                    }, null, 8, ["modelValue", "onUpdate:modelValue", "options", "label"]))
                                  : _createCommentVNode("", true),
                                (_ctx.dialog === 'transfer' && _ctx.targetType !== 'queue')
                                  ? (_openBlock(), _createBlock(_component_q_option_group, {
                                      key: 3,
                                      modelValue: _ctx.mode,
                                      "onUpdate:modelValue": $event => ((_ctx.mode) = $event),
                                      inline: "",
                                      dense: "",
                                      options: [{label: _ctx.t('Now (blind)'), value: 'blind'}, {label: _ctx.t('Talk to them first'), value: 'attended'}]
                                    }, null, 8, ["modelValue", "onUpdate:modelValue", "options"]))
                                  : _createCommentVNode("", true)
                              ]),
                              _: 1
                            })),
                        _createVNode(_component_q_card_actions, { align: "right" }, {
                          default: _withCtx(() => [
                            _createVNode(_component_q_btn, {
                              flat: "",
                              "no-caps": "",
                              label: _ctx.t('Cancel'),
                              onClick: $event => (_ctx.dialog = null)
                            }, null, 8, ["label", "onClick"]),
                            _createVNode(_component_q_btn, {
                              unelevated: "",
                              "no-caps": "",
                              color: "primary",
                              disable: !_ctx.canSend || _ctx.busy,
                              label: _ctx.dialog === 'transfer' ? _ctx.t('Transfer') : _ctx.dialog === 'add' ? _ctx.t('Add') : _ctx.t('Merge'),
                              onClick: _ctx.send
                            }, null, 8, ["disable", "label", "onClick"])
                          ]),
                          _: 1
                        })
                      ]),
                      _: 1
                    })
                  ]),
                  _: 1
                }, 8, ["model-value", "onUpdate:modelValue"])
              ]))
            }
        } },
        "m-call-audio": { hash: "ea53b625", make: function (Vue) {
            const { openBlock: _openBlock, createElementBlock: _createElementBlock } = Vue

            return function render(_ctx, _cache) {
              return (_openBlock(), _createElementBlock("audio", {
                ref: "audio",
                autoplay: ""
              }, null, 512))
            }
        } },
        "m-call-context": { hash: "b23dfddb", make: function (Vue) {
            const { renderList: _renderList, Fragment: _Fragment, openBlock: _openBlock, createElementBlock: _createElementBlock, resolveComponent: _resolveComponent, createVNode: _createVNode, toDisplayString: _toDisplayString, createCommentVNode: _createCommentVNode } = Vue

            return function render(_ctx, _cache) {
              const _component_q_icon = _resolveComponent("q-icon")

              return (_ctx.contacts.length || _ctx.records.length)
                ? (_openBlock(), _createElementBlock("div", {
                    key: 0,
                    class: "m-call-context"
                  }, [
                    (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.contacts, (c) => {
                      return (_openBlock(), _createElementBlock("div", {
                        key: c.sourceId + c.url + c.name,
                        class: "text-caption"
                      }, [
                        _createVNode(_component_q_icon, {
                          name: c.kind === 'organization' ? 'sym_o_domain' : 'sym_o_person',
                          size: "16px",
                          class: "q-mr-xs"
                        }, null, 8, ["name"]),
                        (c.url)
                          ? (_openBlock(), _createElementBlock("a", {
                              key: 0,
                              href: c.url
                            }, _toDisplayString(c.name), 9, ["href"]))
                          : (_openBlock(), _createElementBlock("span", { key: 1 }, _toDisplayString(c.name), 1)),
                        (c.detail)
                          ? (_openBlock(), _createElementBlock("span", {
                              key: 2,
                              class: "m-muted"
                            }, " · " + _toDisplayString(c.detail), 1))
                          : _createCommentVNode("", true)
                      ]))
                    }), 128)),
                    (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.records, (r) => {
                      return (_openBlock(), _createElementBlock("div", {
                        key: r.sourceId + r.url + r.title,
                        class: "text-caption"
                      }, [
                        _createVNode(_component_q_icon, {
                          name: "sym_o_assignment",
                          size: "16px",
                          class: "q-mr-xs"
                        }),
                        (r.url)
                          ? (_openBlock(), _createElementBlock("a", {
                              key: 0,
                              href: r.url
                            }, _toDisplayString(r.title), 9, ["href"]))
                          : (_openBlock(), _createElementBlock("span", { key: 1 }, _toDisplayString(r.title), 1)),
                        (r.status)
                          ? (_openBlock(), _createElementBlock("span", {
                              key: 2,
                              class: "m-muted"
                            }, " · " + _toDisplayString(r.status), 1))
                          : _createCommentVNode("", true)
                      ]))
                    }), 128))
                  ]))
                : _createCommentVNode("", true)
            }
        } },
        "m-call-dock": { hash: "fa997856", make: function (Vue) {
            const { renderList: _renderList, Fragment: _Fragment, openBlock: _openBlock, createElementBlock: _createElementBlock, resolveComponent: _resolveComponent, createBlock: _createBlock, createVNode: _createVNode, toDisplayString: _toDisplayString, createElementVNode: _createElementVNode, createTextVNode: _createTextVNode, createCommentVNode: _createCommentVNode, withCtx: _withCtx } = Vue

            return function render(_ctx, _cache) {
              const _component_m_call_audio = _resolveComponent("m-call-audio")
              const _component_q_icon = _resolveComponent("q-icon")
              const _component_m_call_context = _resolveComponent("m-call-context")
              const _component_q_tooltip = _resolveComponent("q-tooltip")
              const _component_q_btn = _resolveComponent("q-btn")
              const _component_q_card_section = _resolveComponent("q-card-section")
              const _component_q_card = _resolveComponent("q-card")
              const _component_q_page_sticky = _resolveComponent("q-page-sticky")
              const _component_q_banner = _resolveComponent("q-banner")
              const _component_m_call_video = _resolveComponent("m-call-video")
              const _component_q_space = _resolveComponent("q-space")
              const _component_q_card_actions = _resolveComponent("q-card-actions")

              return (_openBlock(), _createElementBlock("div", { class: "m-call-dock" }, [
                (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.remotes, (r) => {
                  return (_openBlock(), _createBlock(_component_m_call_audio, {
                    key: 'a' + r.legId,
                    stream: r.stream,
                    "sink-id": _ctx.call.settings.audioOut
                  }, null, 8, ["stream", "sink-id"]))
                }), 128)),
                (_ctx.ringingCalls.length)
                  ? (_openBlock(), _createBlock(_component_q_page_sticky, {
                      key: 0,
                      position: "top",
                      offset: [0, 8],
                      style: {"z-index":"2100"}
                    }, {
                      default: _withCtx(() => [
                        (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.ringingCalls, (view) => {
                          return (_openBlock(), _createBlock(_component_q_card, {
                            key: view.callSessionId,
                            class: "m-call-ring q-mb-sm",
                            style: {"min-width":"300px"}
                          }, {
                            default: _withCtx(() => [
                              _createVNode(_component_q_card_section, { class: "row items-center no-wrap q-py-sm" }, {
                                default: _withCtx(() => [
                                  _createVNode(_component_q_icon, {
                                    name: view.video ? 'sym_o_videocam' : 'sym_o_call',
                                    size: "28px",
                                    class: "q-mr-sm"
                                  }, null, 8, ["name"]),
                                  _createElementVNode("div", { class: "col" }, [
                                    _createElementVNode("div", { class: "text-subtitle2" }, _toDisplayString(_ctx.fromName(view)), 1),
                                    _createElementVNode("div", { class: "text-caption m-muted" }, [
                                      _createTextVNode(_toDisplayString(view.video ? _ctx.t('Video call') : _ctx.t('Audio call')), 1),
                                      (view.e2ee)
                                        ? (_openBlock(), _createElementBlock("span", { key: 0 }, [
                                            _createTextVNode(" · "),
                                            _createVNode(_component_q_icon, {
                                              name: "sym_o_lock",
                                              size: "14px"
                                            }),
                                            _createTextVNode(" " + _toDisplayString(_ctx.t('End-to-end encrypted')), 1)
                                          ]))
                                        : _createCommentVNode("", true)
                                    ]),
                                    _createVNode(_component_m_call_context, {
                                      "call-session-id": view.callSessionId,
                                      dense: ""
                                    }, null, 8, ["call-session-id"])
                                  ]),
                                  _createVNode(_component_q_btn, {
                                    round: "",
                                    unelevated: "",
                                    color: "positive",
                                    icon: "sym_o_call",
                                    class: "q-ml-sm",
                                    "aria-label": _ctx.answerLabel(view),
                                    onClick: $event => (_ctx.answer(view, view.video))
                                  }, {
                                    default: _withCtx(() => [
                                      _createVNode(_component_q_tooltip, null, {
                                        default: _withCtx(() => [
                                          _createTextVNode(_toDisplayString(_ctx.answerLabel(view)), 1)
                                        ]),
                                        _: 2
                                      }, 1024)
                                    ]),
                                    _: 2
                                  }, 1032, ["aria-label", "onClick"]),
                                  (view.video)
                                    ? (_openBlock(), _createBlock(_component_q_btn, {
                                        key: 0,
                                        round: "",
                                        flat: "",
                                        color: "positive",
                                        icon: "sym_o_mic",
                                        class: "q-ml-xs",
                                        "aria-label": _ctx.answerLabel(view, true),
                                        onClick: $event => (_ctx.answer(view, false))
                                      }, {
                                        default: _withCtx(() => [
                                          _createVNode(_component_q_tooltip, null, {
                                            default: _withCtx(() => [
                                              _createTextVNode(_toDisplayString(_ctx.answerLabel(view, true)), 1)
                                            ]),
                                            _: 2
                                          }, 1024)
                                        ]),
                                        _: 2
                                      }, 1032, ["aria-label", "onClick"]))
                                    : _createCommentVNode("", true),
                                  _createVNode(_component_q_btn, {
                                    round: "",
                                    unelevated: "",
                                    color: "negative",
                                    icon: "sym_o_call_end",
                                    class: "q-ml-xs",
                                    "aria-label": _ctx.t('Decline'),
                                    onClick: $event => (_ctx.decline(view))
                                  }, {
                                    default: _withCtx(() => [
                                      _createVNode(_component_q_tooltip, null, {
                                        default: _withCtx(() => [
                                          _createTextVNode(_toDisplayString(_ctx.t('Decline')), 1)
                                        ]),
                                        _: 1
                                      })
                                    ]),
                                    _: 1
                                  }, 8, ["aria-label", "onClick"])
                                ]),
                                _: 2
                              }, 1024)
                            ]),
                            _: 2
                          }, 1024))
                        }), 128))
                      ]),
                      _: 1
                    }))
                  : _createCommentVNode("", true),
                (_ctx.elsewhere.length && !_ctx.ringingCalls.length && !_ctx.onCallScreen)
                  ? (_openBlock(), _createBlock(_component_q_page_sticky, {
                      key: 1,
                      position: "top",
                      offset: [0, 8],
                      style: {"z-index":"2050"}
                    }, {
                      default: _withCtx(() => [
                        (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.elsewhere, (view) => {
                          return (_openBlock(), _createBlock(_component_q_banner, {
                            key: 'e' + view.callSessionId,
                            dense: "",
                            rounded: "",
                            class: "m-tonal q-mb-sm"
                          }, {
                            avatar: _withCtx(() => [
                              _createVNode(_component_q_icon, { name: "sym_o_call" })
                            ]),
                            action: _withCtx(() => [
                              _createVNode(_component_q_btn, {
                                flat: "",
                                "no-caps": "",
                                label: _ctx.t('Return to the call'),
                                onClick: $event => (_ctx.returnTo(view))
                              }, null, 8, ["label", "onClick"]),
                              (_ctx.canLeaveElsewhere())
                                ? (_openBlock(), _createBlock(_component_q_btn, {
                                    key: 0,
                                    flat: "",
                                    "no-caps": "",
                                    color: "negative",
                                    label: _ctx.t('Leave'),
                                    onClick: $event => (_ctx.leaveElsewhere(view))
                                  }, null, 8, ["label", "onClick"]))
                                : _createCommentVNode("", true)
                            ]),
                            default: _withCtx(() => [
                              _createTextVNode(_toDisplayString(_ctx.t('You are in a call')) + ": " + _toDisplayString(_ctx.fromName(view)), 1)
                            ]),
                            _: 2
                          }, 1024))
                        }), 128))
                      ]),
                      _: 1
                    }))
                  : _createCommentVNode("", true),
                (_ctx.call.lobbyNotice && !_ctx.onCallScreen && !_ctx.ringingCalls.length)
                  ? (_openBlock(), _createBlock(_component_q_page_sticky, {
                      key: 2,
                      position: "top",
                      offset: [0, 8],
                      style: {"z-index":"2060"}
                    }, {
                      default: _withCtx(() => [
                        _createVNode(_component_q_banner, {
                          dense: "",
                          rounded: "",
                          class: "m-tonal"
                        }, {
                          avatar: _withCtx(() => [
                            _createVNode(_component_q_icon, { name: "sym_o_person_add" })
                          ]),
                          action: _withCtx(() => [
                            _createVNode(_component_q_btn, {
                              flat: "",
                              "no-caps": "",
                              label: _ctx.t('Open the call'),
                              onClick: _ctx.openLobby
                            }, null, 8, ["label", "onClick"]),
                            _createVNode(_component_q_btn, {
                              flat: "",
                              round: "",
                              dense: "",
                              icon: "sym_o_close",
                              "aria-label": _ctx.t('Close'),
                              onClick: $event => (_ctx.call.lobbyNotice = null)
                            }, null, 8, ["aria-label", "onClick"])
                          ]),
                          default: _withCtx(() => [
                            _createTextVNode(_toDisplayString(_ctx.call.lobbyNotice.guestName) + " " + _toDisplayString(_ctx.t('is waiting to join your call')), 1)
                          ]),
                          _: 1
                        })
                      ]),
                      _: 1
                    }))
                  : _createCommentVNode("", true),
                (_ctx.call.current && !_ctx.onCallScreen)
                  ? (_openBlock(), _createBlock(_component_q_page_sticky, {
                      key: 3,
                      position: "bottom-right",
                      offset: [16, 16],
                      style: {"z-index":"2000"}
                    }, {
                      default: _withCtx(() => [
                        _createVNode(_component_q_card, {
                          class: "m-live-call",
                          style: {"width":"320px"}
                        }, {
                          default: _withCtx(() => [
                            _createElementVNode("div", {
                              class: "m-live-call-media cursor-pointer",
                              style: {"height":"180px","background":"#000"},
                              onClick: _ctx.back
                            }, [
                              (_ctx.speaker)
                                ? (_openBlock(), _createBlock(_component_m_call_video, {
                                    key: 0,
                                    stream: _ctx.speaker.stream,
                                    muted: "",
                                    style: {"width":"100%","height":"100%","object-fit":"cover"}
                                  }, null, 8, ["stream"]))
                                : (_openBlock(), _createElementBlock("div", {
                                    key: 1,
                                    class: "fit flex flex-center text-white text-subtitle1"
                                  }, _toDisplayString(_ctx.current ? (_ctx.current.participants || []).filter(p => p.inCall).map(p => p.displayName).join(', ') : ''), 1))
                            ], 8, ["onClick"]),
                            _createVNode(_component_q_card_actions, { class: "q-py-xs" }, {
                              default: _withCtx(() => [
                                _createElementVNode("span", { class: "text-caption q-ml-sm" }, _toDisplayString(_ctx.elapsed), 1),
                                _createVNode(_component_q_space),
                                _createVNode(_component_q_btn, {
                                  flat: "",
                                  round: "",
                                  dense: "",
                                  icon: _ctx.call.muted ? 'sym_o_mic_off' : 'sym_o_mic',
                                  "aria-label": _ctx.t('Mute'),
                                  onClick: $event => {$event.stopPropagation(); _ctx.moquiCall().toggleMute()}
                                }, null, 8, ["icon", "aria-label", "onClick"]),
                                _createVNode(_component_q_btn, {
                                  flat: "",
                                  round: "",
                                  dense: "",
                                  icon: _ctx.call.cameraOff ? 'sym_o_videocam_off' : 'sym_o_videocam',
                                  "aria-label": _ctx.t('Camera'),
                                  onClick: $event => (_ctx.moquiCall().toggleCamera())
                                }, null, 8, ["icon", "aria-label", "onClick"]),
                                _createVNode(_component_q_btn, {
                                  flat: "",
                                  round: "",
                                  dense: "",
                                  color: "negative",
                                  icon: "sym_o_call_end",
                                  "aria-label": _ctx.t('Leave'),
                                  onClick: _ctx.leave
                                }, null, 8, ["aria-label", "onClick"])
                              ]),
                              _: 1
                            })
                          ]),
                          _: 1
                        })
                      ]),
                      _: 1
                    }))
                  : _createCommentVNode("", true)
              ]))
            }
        } },
        "m-call-guest": { hash: "61f2b793", make: function (Vue) {
            const { toDisplayString: _toDisplayString, createElementVNode: _createElementVNode, openBlock: _openBlock, createElementBlock: _createElementBlock, createCommentVNode: _createCommentVNode, createTextVNode: _createTextVNode, resolveComponent: _resolveComponent, withCtx: _withCtx, createBlock: _createBlock, createVNode: _createVNode, createSlots: _createSlots, withKeys: _withKeys, renderList: _renderList, Fragment: _Fragment, normalizeStyle: _normalizeStyle } = Vue

            return function render(_ctx, _cache) {
              const _component_q_banner = _resolveComponent("q-banner")
              const _component_q_icon = _resolveComponent("q-icon")
              const _component_q_btn = _resolveComponent("q-btn")
              const _component_m_call_video = _resolveComponent("m-call-video")
              const _component_q_input = _resolveComponent("q-input")
              const _component_q_spinner = _resolveComponent("q-spinner")
              const _component_m_call_audio = _resolveComponent("m-call-audio")

              return (_openBlock(), _createElementBlock("div", {
                class: "m-call-guest column q-gutter-md q-pa-md",
                style: {"max-width":"1100px","margin":"0 auto"}
              }, [
                _createElementVNode("div", null, [
                  _createElementVNode("div", { class: "text-h5" }, _toDisplayString(_ctx.link.name || _ctx.t('Call')), 1),
                  (_ctx.link.ownerName)
                    ? (_openBlock(), _createElementBlock("div", {
                        key: 0,
                        class: "text-caption m-muted"
                      }, _toDisplayString(_ctx.t('With')) + " " + _toDisplayString(_ctx.link.ownerName), 1))
                    : _createCommentVNode("", true)
                ]),
                (_ctx.error)
                  ? (_openBlock(), _createBlock(_component_q_banner, {
                      key: 0,
                      dense: "",
                      rounded: "",
                      class: "m-tonal text-negative"
                    }, {
                      default: _withCtx(() => [
                        _createTextVNode(_toDisplayString(_ctx.error), 1)
                      ]),
                      _: 1
                    }))
                  : _createCommentVNode("", true),
                (_ctx.recording)
                  ? (_openBlock(), _createBlock(_component_q_banner, {
                      key: 1,
                      dense: "",
                      rounded: "",
                      class: "m-tonal m-call-recording-notice"
                    }, _createSlots({
                      avatar: _withCtx(() => [
                        _createVNode(_component_q_icon, {
                          name: "sym_o_fiber_manual_record",
                          color: _ctx.recording.state === 'recording' ? 'negative' : 'warning'
                        }, null, 8, ["color"])
                      ]),
                      default: _withCtx(() => [
                        _createElementVNode("div", { class: "text-weight-medium" }, _toDisplayString(_ctx.moquiRecordingText(_ctx.recording)), 1),
                        _createTextVNode(_toDisplayString(_ctx.phase === 'lobby' || _ctx.phase === 'waiting' ? (_ctx.recording.ackRequired ? _ctx.t('This call is being recorded. Once you are in, you are asked to acknowledge it.') : _ctx.t('This call is being recorded. Joining it is consent.')) : _ctx.withdrawn ? _ctx.t('You withdrew your consent: the recording is paused while you are in the call.') : _ctx.needsAck ? _ctx.t('This call is being recorded. The recording waits until everyone in the call acknowledges.') : _ctx.t('This call is being recorded. Staying in the call is consent.')), 1)
                      ]),
                      _: 2
                    }, [
                      (_ctx.phase === 'in')
                        ? {
                            name: "action",
                            fn: _withCtx(() => [
                              (_ctx.needsAck || _ctx.withdrawn)
                                ? (_openBlock(), _createBlock(_component_q_btn, {
                                    key: 0,
                                    flat: "",
                                    "no-caps": "",
                                    color: "primary",
                                    label: _ctx.withdrawn ? _ctx.t('Consent again') : _ctx.t('Acknowledge'),
                                    onClick: $event => (_ctx.recAction('ackRecording'))
                                  }, null, 8, ["label", "onClick"]))
                                : _createCommentVNode("", true),
                              (!_ctx.withdrawn)
                                ? (_openBlock(), _createBlock(_component_q_btn, {
                                    key: 1,
                                    flat: "",
                                    "no-caps": "",
                                    color: "negative",
                                    label: _ctx.t('Withdraw consent'),
                                    onClick: $event => (_ctx.recAction('withdrawRecording'))
                                  }, null, 8, ["label", "onClick"]))
                                : _createCommentVNode("", true)
                            ]),
                            key: "0"
                          }
                        : undefined
                    ]), 1024))
                  : _createCommentVNode("", true),
                (_ctx.phase === 'lobby' || _ctx.phase === 'waiting')
                  ? (_openBlock(), _createElementBlock("div", {
                      key: 2,
                      class: "row q-col-gutter-md"
                    }, [
                      _createElementVNode("div", { class: "col-12 col-md-7" }, [
                        _createElementVNode("div", { style: {"background":"#000","aspect-ratio":"16/9","border-radius":"8px","overflow":"hidden"} }, [
                          (_ctx.preview && _ctx.preview.getVideoTracks().length)
                            ? (_openBlock(), _createBlock(_component_m_call_video, {
                                key: 0,
                                stream: _ctx.preview,
                                muted: "",
                                mirror: "",
                                style: {"width":"100%","height":"100%","object-fit":"cover"}
                              }, null, 8, ["stream"]))
                            : (_openBlock(), _createElementBlock("div", {
                                key: 1,
                                class: "fit flex flex-center text-white"
                              }, _toDisplayString(_ctx.t('No camera or no permission for it')), 1))
                        ])
                      ]),
                      _createElementVNode("div", { class: "col-12 col-md-5 column q-gutter-sm" }, [
                        _createVNode(_component_q_input, {
                          modelValue: _ctx.name,
                          "onUpdate:modelValue": $event => ((_ctx.name) = $event),
                          outlined: "",
                          dense: "",
                          maxlength: "60",
                          label: _ctx.t('Your name'),
                          disable: _ctx.phase === 'waiting',
                          onKeyup: _withKeys(_ctx.join, ["enter"])
                        }, null, 8, ["modelValue", "onUpdate:modelValue", "label", "disable", "onKeyup"]),
                        (_ctx.link.encrypted)
                          ? (_openBlock(), _createBlock(_component_q_banner, {
                              key: 0,
                              dense: "",
                              rounded: "",
                              class: "m-tonal text-negative"
                            }, {
                              avatar: _withCtx(() => [
                                _createVNode(_component_q_icon, { name: "sym_o_lock" })
                              ]),
                              default: _withCtx(() => [
                                _createTextVNode(_toDisplayString(_ctx.t('This meeting is end-to-end encrypted: only people with an account can join it. Ask its host for another way in.')), 1)
                              ]),
                              _: 1
                            }))
                          : _createCommentVNode("", true),
                        (_ctx.phase === 'lobby')
                          ? (_openBlock(), _createBlock(_component_q_btn, {
                              key: 1,
                              unelevated: "",
                              color: "primary",
                              icon: "sym_o_call",
                              loading: _ctx.busy,
                              disable: !_ctx.name.trim() || !!_ctx.link.encrypted,
                              label: _ctx.link.policy === 'link' ? _ctx.t('Join') : _ctx.t('Ask to join'),
                              onClick: _ctx.join
                            }, null, 8, ["loading", "disable", "label", "onClick"]))
                          : _createCommentVNode("", true),
                        (_ctx.phase === 'waiting')
                          ? (_openBlock(), _createElementBlock("div", {
                              key: 2,
                              class: "row items-center q-gutter-sm"
                            }, [
                              _createVNode(_component_q_spinner, { size: "20px" }),
                              _createElementVNode("span", null, _toDisplayString(_ctx.t('Waiting for someone in the call to let you in')), 1)
                            ]))
                          : _createCommentVNode("", true)
                      ])
                    ]))
                  : _createCommentVNode("", true),
                (_ctx.phase === 'in')
                  ? (_openBlock(), _createElementBlock("div", { key: 3 }, [
                      _createElementVNode("div", {
                        style: _normalizeStyle(_ctx.gridStyle)
                      }, [
                        _createElementVNode("div", { style: {"position":"relative","background":"#000","aspect-ratio":"16/9","border-radius":"8px","overflow":"hidden"} }, [
                          (_ctx.call.localStream && _ctx.call.localStream.getVideoTracks().length && !_ctx.call.cameraOff)
                            ? (_openBlock(), _createBlock(_component_m_call_video, {
                                key: 0,
                                stream: _ctx.call.localStream,
                                muted: "",
                                mirror: "",
                                style: {"width":"100%","height":"100%","object-fit":"cover"}
                              }, null, 8, ["stream"]))
                            : (_openBlock(), _createElementBlock("div", {
                                key: 1,
                                class: "fit flex flex-center text-white"
                              }, _toDisplayString(_ctx.name), 1)),
                          _createElementVNode("div", { class: "absolute-bottom-left q-pa-xs text-white text-caption" }, _toDisplayString(_ctx.name), 1)
                        ]),
                        (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.remotes, (r) => {
                          return (_openBlock(), _createElementBlock("div", {
                            key: r.legId,
                            style: {"position":"relative","background":"#000","aspect-ratio":"16/9","border-radius":"8px","overflow":"hidden"}
                          }, [
                            ((r.screen || r.stream) && (r.screen || r.stream).getVideoTracks().length && !r.cameraOff)
                              ? (_openBlock(), _createBlock(_component_m_call_video, {
                                  key: 0,
                                  stream: r.screen || r.stream,
                                  muted: "",
                                  style: {"width":"100%","height":"100%","object-fit":"cover"}
                                }, null, 8, ["stream"]))
                              : (_openBlock(), _createElementBlock("div", {
                                  key: 1,
                                  class: "fit flex flex-center text-white"
                                }, _toDisplayString(r.name), 1)),
                            _createVNode(_component_m_call_audio, {
                              stream: r.stream,
                              "sink-id": _ctx.call.settings.audioOut
                            }, null, 8, ["stream", "sink-id"]),
                            _createElementVNode("div", { class: "absolute-bottom-left q-pa-xs text-white text-caption" }, _toDisplayString(r.name), 1)
                          ]))
                        }), 128))
                      ], 4),
                      _createElementVNode("div", { class: "text-caption m-muted q-mt-xs" }, _toDisplayString(_ctx.people.map(p => p.displayName).join(', ')), 1),
                      (true && (_ctx.call.captions.on || _ctx.call.captions.unavailable))
                        ? (_openBlock(), _createElementBlock("div", {
                            key: 0,
                            class: "m-call-captions q-pa-sm rounded-borders",
                            style: {"background":"rgba(0,0,0,.78)","color":"#fff"},
                            "aria-live": "polite"
                          }, [
                            _createElementVNode("div", {
                              class: "text-caption",
                              style: {"opacity":".85"}
                            }, [
                              _createVNode(_component_q_icon, {
                                name: "sym_o_closed_caption",
                                class: "q-mr-xs"
                              }),
                              _createTextVNode(_toDisplayString(_ctx.captionNotice), 1)
                            ]),
                            (_ctx.call.captions.unavailable)
                              ? (_openBlock(), _createElementBlock("div", {
                                  key: 0,
                                  class: "text-caption text-warning"
                                }, [
                                  _createVNode(_component_q_icon, {
                                    name: "sym_o_warning",
                                    class: "q-mr-xs"
                                  }),
                                  _createTextVNode(_toDisplayString(_ctx.t('Captions are unavailable')) + ": " + _toDisplayString(_ctx.call.captions.unavailable), 1)
                                ]))
                              : _createCommentVNode("", true),
                            (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.call.captions.lines, (line) => {
                              return (_openBlock(), _createElementBlock("div", {
                                key: line.key,
                                class: "text-body2"
                              }, [
                                _createElementVNode("span", { class: "text-weight-bold" }, _toDisplayString(line.name) + ":", 1),
                                _createTextVNode(),
                                _createElementVNode("span", {
                                  style: _normalizeStyle(line.gap ? 'opacity:.6' : '')
                                }, _toDisplayString(line.text), 5)
                              ]))
                            }), 128))
                          ]))
                        : _createCommentVNode("", true),
                      _createElementVNode("div", { class: "row justify-center q-gutter-sm q-mt-sm" }, [
                        _createVNode(_component_q_btn, {
                          round: "",
                          unelevated: "",
                          color: _ctx.call.muted ? 'negative' : 'grey-8',
                          icon: _ctx.call.muted ? 'sym_o_mic_off' : 'sym_o_mic',
                          "aria-label": _ctx.t('Mute'),
                          onClick: $event => (_ctx.moquiCall().toggleMute())
                        }, null, 8, ["color", "icon", "aria-label", "onClick"]),
                        _createVNode(_component_q_btn, {
                          round: "",
                          unelevated: "",
                          color: _ctx.call.cameraOff ? 'negative' : 'grey-8',
                          icon: _ctx.call.cameraOff ? 'sym_o_videocam_off' : 'sym_o_videocam',
                          "aria-label": _ctx.t('Camera'),
                          onClick: $event => (_ctx.moquiCall().toggleCamera())
                        }, null, 8, ["color", "icon", "aria-label", "onClick"]),
                        _createVNode(_component_q_btn, {
                          round: "",
                          unelevated: "",
                          color: _ctx.call.hand ? 'warning' : 'grey-8',
                          icon: "sym_o_back_hand",
                          "aria-label": _ctx.t('Raise your hand'),
                          onClick: $event => (_ctx.moquiCall().toggleHand())
                        }, null, 8, ["color", "aria-label", "onClick"]),
                        _createVNode(_component_q_btn, {
                          round: "",
                          unelevated: "",
                          color: "negative",
                          icon: "sym_o_call_end",
                          "aria-label": _ctx.t('Leave'),
                          onClick: _ctx.leave
                        }, null, 8, ["aria-label", "onClick"])
                      ])
                    ]))
                  : _createCommentVNode("", true),
                (_ctx.phase === 'denied')
                  ? (_openBlock(), _createElementBlock("div", {
                      key: 4,
                      class: "text-subtitle1"
                    }, _toDisplayString(_ctx.t('You were not let in to this call.')), 1))
                  : _createCommentVNode("", true),
                (_ctx.phase === 'ended')
                  ? (_openBlock(), _createElementBlock("div", {
                      key: 5,
                      class: "text-subtitle1"
                    }, _toDisplayString(_ctx.t('The call has ended for you.')), 1))
                  : _createCommentVNode("", true),
                (_ctx.phase === 'left')
                  ? (_openBlock(), _createElementBlock("div", {
                      key: 6,
                      class: "text-subtitle1"
                    }, _toDisplayString(_ctx.t('You left the call.')), 1))
                  : _createCommentVNode("", true)
              ]))
            }
        } },
        "m-call-video": { hash: "f8b0eee5", make: function (Vue) {
            const { normalizeStyle: _normalizeStyle, openBlock: _openBlock, createElementBlock: _createElementBlock } = Vue

            return function render(_ctx, _cache) {
              return (_openBlock(), _createElementBlock("video", {
                ref: "video",
                autoplay: "",
                playsinline: "",
                muted: _ctx.muted,
                class: "m-call-video",
                style: _normalizeStyle(_ctx.mirror ? 'transform:scaleX(-1)' : '')
              }, null, 12, ["muted"]))
            }
        } }
    });
})(globalThis.moqui);

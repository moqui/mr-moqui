/*
 * This software is in the public domain under CC0 1.0 Universal plus a
 * Grant of Patent License.
 *
 * To the extent possible under law, the author(s) have dedicated all
 * copyright and related and neighboring rights to this software to the
 * public domain worldwide. This software is distributed without any
 * warranty.
 *
 * You should have received a copy of the CC0 Public Domain Dedication
 * along with this software (see the LICENSE.md file). If not, see
 * <http://creativecommons.org/publicdomain/zero/1.0/>.
 */

//! URL objects for `sri.buildUrl` and `sri.makeUrlByType`.

use rquickjs::object::Accessor;

use super::screen_host::{lit_of, with_render};
use super::*;
use crate::screen::urls::{self, UrlInstance};

/// A URL a link is made from. The methods are the ones the macros call on a Java `UrlInstance`.
#[derive(Trace, JsLifetime)]
#[rquickjs::class]
pub(super) struct HostUrl {
    #[qjs(skip_trace)]
    inner: RefCell<UrlInstance>,
}

#[rquickjs::methods(rename_all = "camelCase")]
impl HostUrl {
    #[qjs(get, rename = "path")]
    fn path_property(&self, ctx: Ctx<'_>) -> rquickjs::Result<String> {
        host_guard(&ctx, || Ok(self.inner.borrow().path()))
    }

    #[qjs(get, rename = "url")]
    fn url_property(&self, ctx: Ctx<'_>) -> rquickjs::Result<String> {
        host_guard(&ctx, || Ok(self.inner.borrow().url()))
    }

    #[qjs(get, rename = "pathWithParams")]
    fn path_with_params_property(&self, ctx: Ctx<'_>) -> rquickjs::Result<String> {
        host_guard(&ctx, || Ok(self.inner.borrow().path_with_params()))
    }

    #[qjs(get, rename = "readOnly")]
    fn read_only_property(&self, ctx: Ctx<'_>) -> rquickjs::Result<bool> {
        host_guard(&ctx, || Ok(self.inner.borrow().read_only))
    }

    fn get_path(&self, ctx: Ctx<'_>) -> rquickjs::Result<String> {
        host_guard(&ctx, || Ok(self.inner.borrow().path()))
    }

    fn get_url(&self, ctx: Ctx<'_>) -> rquickjs::Result<String> {
        host_guard(&ctx, || Ok(self.inner.borrow().url()))
    }

    fn get_path_with_params(&self, ctx: Ctx<'_>) -> rquickjs::Result<String> {
        host_guard(&ctx, || Ok(self.inner.borrow().path_with_params()))
    }

    fn get_url_with_params(&self, ctx: Ctx<'_>, extension: Opt<String>) -> rquickjs::Result<String> {
        host_guard(&ctx, || {
            Ok(self
                .inner
                .borrow()
                .url_with_params(extension.0.as_deref().unwrap_or("")))
        })
    }

    fn get_minimal_path_url(&self, ctx: Ctx<'_>) -> rquickjs::Result<String> {
        host_guard(&ctx, || Ok(self.inner.borrow().minimal_path_url()))
    }

    fn is_read_only(&self, ctx: Ctx<'_>) -> rquickjs::Result<bool> {
        host_guard(&ctx, || Ok(self.inner.borrow().read_only))
    }

    /// A screen or transition of this server, not a plain URL.
    fn is_screen_url(&self, ctx: Ctx<'_>) -> rquickjs::Result<bool> {
        host_guard(&ctx, || Ok(self.inner.borrow().plain.is_none()))
    }

    fn is_transition(&self, ctx: Ctx<'_>) -> rquickjs::Result<bool> {
        host_guard(&ctx, || Ok(self.inner.borrow().is_transition))
    }

    fn get_transition_method(&self, ctx: Ctx<'_>) -> rquickjs::Result<String> {
        host_guard(&ctx, || Ok(self.inner.borrow().transition_method.clone()))
    }

    fn get_disable_link(&self, ctx: Ctx<'_>) -> rquickjs::Result<bool> {
        host_guard(&ctx, || {
            let exists = self.inner.borrow().exists;
            let disabled = self
                .inner
                .borrow()
                .disable_link()
                .map_err(|err| Exception::throw_message(&ctx, err.message()))?;
            if !exists {
                // A static template does not write a disabled link, so a report records that the merged page did.
                crate::screen::statics::record_note(
                    "disable-link",
                    "link",
                    &format!("{} is not a screen or transition", self.inner.borrow().path()),
                );
            }
            Ok(disabled)
        })
    }

    fn clone_url_instance<'js>(&self, ctx: Ctx<'js>) -> rquickjs::Result<Class<'js, HostUrl>> {
        host_guard(&ctx, || wrap(&ctx, self.inner.borrow().clone()))
    }

    /// Returns the URL, so calls chain as they do on a Java `UrlInstance`.
    fn add_parameter<'js>(
        this: This<Class<'js, HostUrl>>,
        ctx: Ctx<'js>,
        name: JsValue<'js>,
        value: JsValue<'js>,
    ) -> rquickjs::Result<Class<'js, HostUrl>> {
        host_guard(&ctx, || {
            let name = js_to_host(&ctx, &name)?;
            let value = js_to_host(&ctx, &value)?;
            this.0
                .borrow()
                .inner
                .borrow_mut()
                .add_parameter(&moqui_lib::object::to_plain_string(&name), &value);
            Ok(this.0.clone())
        })
    }

    fn add_parameters<'js>(
        this: This<Class<'js, HostUrl>>,
        ctx: Ctx<'js>,
        map: JsValue<'js>,
    ) -> rquickjs::Result<Class<'js, HostUrl>> {
        host_guard(&ctx, || {
            if let Value::Map(entries) = js_to_host(&ctx, &map)? {
                let url = this.0.borrow();
                for (key, value) in entries {
                    if key != "context" {
                        url.inner.borrow_mut().add_parameter(&key, &value);
                    }
                }
            }
            Ok(this.0.clone())
        })
    }

    fn remove_parameter<'js>(
        this: This<Class<'js, HostUrl>>,
        ctx: Ctx<'js>,
        name: String,
    ) -> rquickjs::Result<Class<'js, HostUrl>> {
        host_guard(&ctx, || {
            this.0.borrow().inner.borrow_mut().remove_parameter(&name);
            Ok(this.0.clone())
        })
    }

    fn get_parameter_map<'js>(&self, ctx: Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        host_guard(&ctx, || strings_to_js(&ctx, &self.inner.borrow().parameters))
    }

    fn get_pass_through_parameter_map<'js>(&self, ctx: Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        host_guard(&ctx, || {
            strings_to_js(&ctx, &self.inner.borrow().pass_through_parameters())
        })
    }
}

fn strings_to_js<'js>(ctx: &Ctx<'js>, map: &indexmap::IndexMap<String, String>) -> rquickjs::Result<JsValue<'js>> {
    let object = Object::new(ctx.clone())?;
    for (key, value) in map {
        object.set(key.as_str(), value.as_str())?;
    }
    Ok(object.into_value())
}

fn wrap<'js>(ctx: &Ctx<'js>, url: UrlInstance) -> rquickjs::Result<Class<'js, HostUrl>> {
    Class::instance(
        ctx.clone(),
        HostUrl {
            inner: RefCell::new(url),
        },
    )
}

fn sri_build_url<'js>(ctx: Ctx<'js>, path: String) -> rquickjs::Result<Class<'js, HostUrl>> {
    host_guard(&ctx, || {
        let url = with_render(&ctx, false, |state| urls::build_url(state, &path))?;
        wrap(&ctx, url)
    })
}

fn sri_build_url_from_target<'js>(ctx: Ctx<'js>, path: String) -> rquickjs::Result<Class<'js, HostUrl>> {
    host_guard(&ctx, || {
        let url = with_render(&ctx, false, |state| urls::build_url_from_target(state, &path))?;
        wrap(&ctx, url)
    })
}

fn sri_current_url<'js>(ctx: Ctx<'js>) -> rquickjs::Result<Class<'js, HostUrl>> {
    host_guard(&ctx, || {
        let url = with_render(&ctx, false, urls::current_url)?;
        wrap(&ctx, url)
    })
}

fn sri_make_url_by_type<'js>(
    ctx: Ctx<'js>,
    url: String,
    url_type: String,
    parameter_parent: Opt<JsValue<'js>>,
    expand_transition_url: Opt<String>,
) -> rquickjs::Result<Class<'js, HostUrl>> {
    host_guard(&ctx, || {
        let parent = match parameter_parent.0.as_ref().filter(|value| value.is_object()) {
            Some(value) => Some(lit_of(&ctx, value)?),
            None => None,
        };
        let made = with_render(&ctx, false, |state| {
            // Java's `"true".equals(expandTransitionUrlString)`: only an explicit "true" expands an alias transition.
            let expand = expand_transition_url.0.as_deref() == Some("true");
            urls::make_url_by_type(state, &url, &url_type, parent.as_ref(), expand)
        })?;
        wrap(&ctx, made)
    })
}

fn sri_is_anchor_link<'js>(ctx: Ctx<'js>, link: JsValue<'js>, url: Class<'js, HostUrl>) -> rquickjs::Result<bool> {
    host_guard(&ctx, || {
        let link = lit_of(&ctx, &link)?;
        let instance = url.borrow().inner.borrow().clone();
        Ok(urls::is_anchor_link(&link, &instance))
    })
}

pub(super) fn install<'js>(ctx: &Ctx<'js>, sri: &Object<'js>) -> Result<(), Error> {
    macro_rules! method {
        ($name:literal, $function:expr) => {
            let function = caught(ctx, $name, Function::new(ctx.clone(), $function))?;
            caught(ctx, $name, sri.set($name, function))?;
        };
    }
    method!("buildUrl", sri_build_url);
    method!("buildUrlFromTarget", sri_build_url_from_target);
    method!("getScreenUrlInstance", sri_current_url);
    method!("getCurrentScreenUrl", sri_current_url);
    method!("makeUrlByType", sri_make_url_by_type);
    method!("isAnchorLink", sri_is_anchor_link);
    let active = Accessor::from(|ctx: Ctx<'js>| -> rquickjs::Result<bool> {
        host_guard(&ctx, || {
            with_render(&ctx, false, |state| Ok(urls::active_in_current_menu(state)))
        })
    })
    .enumerable();
    caught(ctx, "activeInCurrentMenu", sri.prop("activeInCurrentMenu", active))
}

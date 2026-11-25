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

//! A rename's family of names in the memory index (plans/design-dev-aide.md 13.6, `moqui lsp`).

use super::{resolve, MemoryIndex};

impl MemoryIndex {
    /// The artifacts that share the name of the entity field, parameter or form field `id` and go with it in a rename:
    /// `id`, what takes its name from it, in turn ([`Resolver::same_name_dependents`]), and the form fields bound by
    /// their name into a parameter of them. What a rename cannot follow is `blocking`: one of them read by another name,
    /// or a form field of them that posts to a parameter that is not of them.
    #[must_use]
    pub fn name_family(&self, id: &str) -> resolve::NameDependents {
        let mut family = resolve::NameDependents {
            ids: vec![id.to_string()],
            blocking: Vec::new(),
        };
        let mut at = 0;
        while at < family.ids.len() {
            let current = family.ids[at].clone();
            at += 1;
            let mut next = self.resolver.same_name_dependents(&current);
            family.blocking.append(&mut next.blocking);
            if current.starts_with("parameter:") {
                // the form fields that post to it by their name
                next.ids.extend(
                    self.bindings
                        .iter()
                        .filter(|(_, binding)| binding.to_id == current && binding.source_kind == "form-field")
                        .filter_map(|(_, binding)| binding.source_id.clone()),
                );
            }
            for found in next.ids {
                if !family.ids.contains(&found) {
                    family.ids.push(found);
                }
            }
        }
        for (_, binding) in &self.bindings {
            let from_family = binding
                .source_id
                .as_ref()
                .is_some_and(|source| binding.source_kind == "form-field" && family.ids.contains(source));
            if from_family && !family.ids.contains(&binding.to_id) {
                family.blocking.push(format!(
                    "the form field {} posts to {}",
                    binding.source_text, binding.to_id
                ));
            }
        }
        family.blocking.sort();
        family.blocking.dedup();
        family
    }
}

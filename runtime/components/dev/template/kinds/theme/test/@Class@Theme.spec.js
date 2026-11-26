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
// The theme of @Title@ (data/@Class@ThemeData.xml): built on the Default theme, its own style sheet last, and its color
// scheme its default. Run: ./moq test @component@

feature("the theme builds on the Default theme and ends with its own style sheet", {
    expect() {
        const find = (entity) => ec.entity.find(entity).condition("screenThemeId", "@ID@_QUASAR").disableAuthz()
        const theme = find("moqui.screen.ScreenTheme").one()
        const sheets = find("moqui.screen.ScreenThemeResource").condition("resourceTypeEnumId", "STRT_STYLESHEET")
            .orderBy("sequenceNum").list()
        theme.parentScreenThemeId == "DEFAULT_QUASAR"
        sheets[sheets.length - 1].resourceValue == "/@component@/@Class@.css"
        find("moqui.screen.ScreenColorScheme").list().some((scheme) => scheme.colorSchemeId == "@ID@")
    },
})

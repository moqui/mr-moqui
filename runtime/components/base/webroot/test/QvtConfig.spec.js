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

// The shell's settings builder restores the saved choice without mistaking permission to choose for a forced mode.
// No screen render or database is needed for the policy/choice precedence rule.
spec({database: false})

function configFor(policy, savedMode) {
    const configModule = runInContext(ec.resource.getLocationText('component://webroot/screen/includes/QvtConfig.js'), {
        System: {getProperty: () => ''}
    })
    const preferences = {QUASAR_DARK: savedMode}
    const appearance = {policy}
    const service = {name() { return this }, parameter() { return this }, disableAuthz() { return this }, call: () => ({appearance})}
    const find = {condition() { return this }, disableAuthz() { return this }, list: () => []}
    const context = {
        user: {userId: 'UI_TEST', username: 'ui.test', userAccount: {}, getPreference: key => preferences[key] || null},
        entity: {find: () => find}, service: {sync: () => service}, context: {sharedMap: {}},
        web: {getHostName: () => 'localhost'}, l10n: {localize: text => text}, factory: {moquiVersion: 'test'},
        logger: {error: message => { throw new Error(message) }}
    }
    const sri = {getThemeValues: () => [], getCurrentThemeId: () => 'DEFAULT_QUASAR', buildUrl: path => ({url: path})}
    return configModule.build(context, sri, {localeTag: 'en-US', quasarLang: 'en-US'})
}

feature('the shell restores saved dark, light and automatic choices when mode changes are allowed', {
    expect() {
        for (const [saved, expected] of [['true', true], ['false', false], ['auto', 'auto']]) {
            assert.equal(configFor({mode: true, lockedKeys: []}, saved).darkMode, expected)
            assert.equal(configFor({}, saved).darkMode, expected)
        }
    }
})

feature('a forced or locked mode takes precedence over the saved choice', {
    expect() {
        for (const [mode, expected] of [['dark', true], ['light', false], ['auto', 'auto']]) {
            assert.equal(configFor({mode, lockedKeys: ['QUASAR_DARK']}, 'true').darkMode, expected)
        }
        assert.equal(configFor({mode: false, lockedKeys: ['QUASAR_DARK']}, 'true').darkMode, false)
    }
})

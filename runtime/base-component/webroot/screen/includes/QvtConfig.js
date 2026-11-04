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

// The settings of the qvt client (docs/ui/Qvt-Client.md): the /qapps page writes them as one JSON block in the shell
// (qapps.xml, WebrootVue.qvt.html.fancy) and the desktop client gets them from its clientConfig transition, so both agree.
// No session token here: the page adds its own; the desktop client signs in by key and needs none.
//
// options: staticOnly (the runtime build of Vue, server-static templates only), localeTag, quasarLang, rumSettings.
function build(ec, sri, options) {
    const staticOnly = !!options.staticOnly
    const localeTag = options.localeTag
    const quasarLang = options.quasarLang
    const rumSettings = options.rumSettings || null
    const qzEnabled = ec.user.getPreference('qz.print.enabled') == 'true'
    // The snapshot of the user for the browser ec of a server-static screen: the account without its password columns, and the
    // preferences the user has set (ec.user.getPreference falls back to group and default values, so each is read through it).
    const account = ec.user.userAccount || {}
    const preferences = {}
    if (ec.user.userId) {
        const found = ec.entity.find('moqui.security.UserPreference').condition('userId', ec.user.userId).disableAuthz().list()
        for (let i = 0; i < found.length; i++) {
            const key = found.get(i).get('preferenceKey')
            preferences[key] = ec.user.getPreference(key)
        }
    }
    // SHELL_START_APP: the app /qapps opens on with no path, when the user may open it (the apps of the kept shell providers);
    // without the user's choice, the AppList
    // SHELL_DENSITY: comfortable (the default) or compact (docs/ui/Qvt-Client.md, Preferences)
    let startApp = ''
    const startPref = ec.user.userId ? ec.user.getPreference('SHELL_START_APP') : null
    if (startPref) {
        try {
            if (sri.getShellProviders('notificationTopics').apps.some((app) => app.name === startPref)) startApp = startPref
        } catch (e) { ec.logger.warn('Start app ' + startPref + ' not checked: ' + e) }
    }
    const logoList = sri.getThemeValues('STRT_HEADER_LOGO')
    const titleList = sri.getThemeValues('STRT_HEADER_TITLE')
    // STRT_AVATAR_URL is a template with the characters ${userId}. Do not expand it here: the browser replaces the user id.
    const avatarList = sri.getThemeValues('STRT_AVATAR_URL')
    // The Quasar configuration of the theme (brand colors, notify, loadingBar) as JSON in a STRT_QUASAR_CONFIG resource; a bad one is logged and skipped.
    let quasarConfig = null
    for (const text of sri.getThemeValues('STRT_QUASAR_CONFIG')) {
        try { quasarConfig = Object.assign(quasarConfig || {}, JSON.parse(text)) } catch (e) { ec.logger.error('Theme STRT_QUASAR_CONFIG is not JSON: ' + e) }
    }
    // Appearance (docs/ui/Qvt-Client.md, Appearance): the theme's color schemes and fonts, the user's choices the theme's policy
    // allows, the effective scheme and the style sheets the page links before the client starts, after the theme's
    // STRT_APPEARANCE_SERVICE extensions (org.moqui.impl.ShellServices.get#Appearance). A failure is logged and the shell keeps its own look.
    let appearance = { colorSchemes: [], fonts: [], values: {}, links: [], policy: {}, keys: [], tokens: {} }
    try {
        appearance = ec.service.sync().name('org.moqui.impl.ShellServices.get#Appearance')
                .parameter('screenThemeId', sri.getCurrentThemeId()).disableAuthz().call().appearance || appearance
    } catch (e) { ec.logger.error('Appearance of theme ' + sri.getCurrentThemeId() + ' not built: ' + e) }
    const lockedKeys = (appearance.policy && appearance.policy.lockedKeys) || []
    // the theme's forced mode wins; a locked mode or density is the shell's default, not what the user chose before
    // A boolean mode is the permission to choose, not a forced color mode.
    const policyMode = appearance.policy && appearance.policy.mode
    const forcedMode = ['light', 'dark', 'auto'].includes(policyMode) ? policyMode : ''
    const darkPref = forcedMode ? (forcedMode === 'dark' ? 'true' : forcedMode === 'auto' ? 'auto' : 'false')
            : lockedKeys.includes('QUASAR_DARK') ? null : ec.user.getPreference('QUASAR_DARK')
    const density = (ec.user.userId && !lockedKeys.includes('SHELL_DENSITY') && ec.user.getPreference('SHELL_DENSITY')) === 'compact' ? 'compact' : 'comfortable'
    return {
        pageTraceId: ec.context.sharedMap.telemetryPageTraceId || '', rum: rumSettings,
        appHost: ec.web.getHostName(true), appRootPath: '', basePath: '/apps', linkBasePath: '/qapps',
        userId: ec.user.userId, username: ec.user.username, userFullName: (ec.user.userAccount && ec.user.userAccount.userFullName) || '',
        locale: localeTag, timeZone: ec.user.timeZone, quasarLang: quasarLang.replace(/-([a-zA-Z])/g, (m, ch) => ch.toUpperCase()),
        darkMode: darkPref == 'auto' ? 'auto' : darkPref == 'true', leftOpen: ec.user.getPreference('QUASAR_LEFT_OPEN') == 'true',
        leftMini: ec.user.getPreference('QUASAR_LEFT_MINI') == 'true',
        density: density, startApp: startApp,
        notificationEndpoint: (ec.user.userId && System.getProperty('webapp_notification_endpoint')) || '',
        // calls (js/WebrootCall.js): the Comms app the call links and transitions are under, for a signed-in user with the socket
        calls: (ec.user.userId && System.getProperty('webapp_notification_endpoint')) ? { appRoot: (System.getProperty('comms_app_root') || '/qapps/BasicComms').replace(/\/+$/, '') } : null,
        // the account menu's Terminal, when the host serves the browser terminal (the /termws endpoint)
        terminalUrl: (ec.user.userId && System.getProperty('webapp_terminal_endpoint')) ? '/qapps/Terminal' : '',
        navPlugins: sri.getThemeValues('STRT_HEADER_NAVBAR_COMP'), accountPlugins: sri.getThemeValues('STRT_HEADER_ACCOUNT_COMP'),
        qz: { enabled: qzEnabled },
        // Assist (js/WebrootAssist.js): the agents are built and on, and the user may use them (AgentUse); else the drawer makes no request
        assist: !!(ec.user.userId && ec.llm && ec.llm.isEnabled() && ec.user.hasPermission('AgentUse')),
        userAccount: { partyId: account.partyId || '', emailAddress: account.emailAddress || '', userFullName: account.userFullName || '' },
        appearance: appearance,
        // the dev/test instance marker (the webapp_dev_marker_color conf property): the shell draws a small square in the corner
        devMarker: System.getProperty('webapp_dev_marker_color') || '',
        preferences: preferences, moquiVersion: ec.factory.moquiVersion, quasar: quasarConfig,
        avatarUrl: avatarList.length ? String(avatarList[0]) : '',
        qvt: { build: staticOnly ? 'runtime' : 'compiler', handles: staticOnly ? ['static-template'] : ['merged-fragment', 'static-template'] },
        // the shell (m-qapps-root): the theme's logo, title and footer, the logout URL, and its labels in the user's language
        shell: { logoUrl: logoList.length ? sri.buildUrl(logoList[0]).url : '', title: titleList.length ? ec.resource.expand(titleList[0], '') : '',
            logoutUrl: sri.buildUrl('/Login/logout').url, footerHtml: sri.getThemeValues('STRT_FOOTER_ITEM').map((item) => sri.renderInlineText(item)).join('') },
        labels: { sendCodeTo: ec.l10n.localize('Send code to'), documentation: ec.l10n.localize('Documentation'), notifyHistory: ec.l10n.localize('Notify History'),
            screenHistory: ec.l10n.localize('Screen History'), account: ec.l10n.localize('Account'), welcome: ec.l10n.localize('Welcome'),
            logout: ec.l10n.localize('Logout'), switchDarkLight: ec.l10n.localize('Switch Dark/Light'), reLogin: ec.l10n.localize('Re-Login'),
            authenticationCode: ec.l10n.localize('Authentication Code'), signIn: ec.l10n.localize('Sign in'), password: ec.l10n.localize('Password'),
            reloadPage: ec.l10n.localize('Reload Page'), terminal: ec.l10n.localize('Terminal UI'),
            // the suite shell: launcher, search and palette, quick create, the tray and its counts, the keys (docs/ui/Qvt-Client.md)
            menu: ec.l10n.localize('Menu'), applications: ec.l10n.localize('Applications'), allApplications: ec.l10n.localize('All applications'), search: ec.l10n.localize('Search'),
            quickCreate: ec.l10n.localize('Create'), quickCreateFirst: ec.l10n.localize('Create in this app'), today: ec.l10n.localize('Today'),
            chat: ec.l10n.localize('Chat'), mail: ec.l10n.localize('Mail'), messages: ec.l10n.localize('Messages'), notifications: ec.l10n.localize('Notifications'), close: ec.l10n.localize('Close'),
            closeTopmost: ec.l10n.localize('Close the dialog, menu or panel'), keyboardShortcuts: ec.l10n.localize('Keyboard shortcuts'),
            keysEverywhere: ec.l10n.localize('Everywhere'), recent: ec.l10n.localize('Recent'), screens: ec.l10n.localize('Screens'),
            savedFinds: ec.l10n.localize('Saved finds'), actions: ec.l10n.localize('Actions'), more: ec.l10n.localize('More'),
            noResults: ec.l10n.localize('Nothing found'), paletteHint: ec.l10n.localize('Search screens, records and actions'),
            paletteEmpty: ec.l10n.localize('Type to search screens, records, people and actions'),
            paletteKeys: ec.l10n.localize('↑ ↓ to move, Enter to open, Ctrl+Enter in a new tab'),
            // the account menu's status (js/WebrootChat.js)
            statusOnline: ec.l10n.localize('Online'), statusAway: ec.l10n.localize('Away'), statusDnd: ec.l10n.localize('Do not disturb'),
            statusInvisible: ec.l10n.localize('Invisible'), statusOffline: ec.l10n.localize('Offline'),
            setStatus: ec.l10n.localize('Set a status message'), clearStatus: ec.l10n.localize('Clear status message'),
            statusText: ec.l10n.localize("What's your status?"), emoji: ec.l10n.localize('Emoji'), clearAfter: ec.l10n.localize('Clear after'),
            dontClear: ec.l10n.localize("Don't clear"), minutes30: ec.l10n.localize('30 minutes'), hour1: ec.l10n.localize('1 hour'),
            hours4: ec.l10n.localize('4 hours'), thisWeek: ec.l10n.localize('This week'),
            save: ec.l10n.localize('Save'), cancel: ec.l10n.localize('Cancel'),
            // the Appearance dialog (m-appearance-dialog)
            appearance: ec.l10n.localize('Appearance'), appearanceMode: ec.l10n.localize('Mode'), modeLight: ec.l10n.localize('Light'),
            modeDark: ec.l10n.localize('Dark'), modeAuto: ec.l10n.localize('Auto (system)'), colorScheme: ec.l10n.localize('Color scheme'),
            accentColor: ec.l10n.localize('Accent color'), schemeColor: ec.l10n.localize('Scheme color'), fonts: ec.l10n.localize('Fonts'),
            fontBody: ec.l10n.localize('Text'), fontHeading: ec.l10n.localize('Headings'), fontMono: ec.l10n.localize('Code'),
            themeDefault: ec.l10n.localize('Theme default'), textSize: ec.l10n.localize('Text size'),
            sizeSmall: ec.l10n.localize('Small text (tables, captions)'), sizeLarge: ec.l10n.localize('Large text (headings)'),
            density: ec.l10n.localize('Density'), densityComfortable: ec.l10n.localize('Comfortable'), densityCompact: ec.l10n.localize('Compact'),
            motion: ec.l10n.localize('Motion'), motionAuto: ec.l10n.localize('Follow the system'), motionReduce: ec.l10n.localize('Reduce'),
            motionFull: ec.l10n.localize('Full'), readability: ec.l10n.localize('Readability'), lineRelaxed: ec.l10n.localize('Relaxed line spacing'),
            linksUnderline: ec.l10n.localize('Underline links'), corners: ec.l10n.localize('Corners'), cornersDefault: ec.l10n.localize('Rounded'),
            cornersSquare: ec.l10n.localize('Square'), cornersRound: ec.l10n.localize('More rounded'), resetDefaults: ec.l10n.localize('Reset to defaults'),
            done: ec.l10n.localize('Done'),
            // the account menu's Notifications on this device (js/WebrootPush.js)
            pushNotifications: ec.l10n.localize('Notifications on this device'), pushBlocked: ec.l10n.localize("Blocked in the browser's site settings"),
            pushBlockedHelp: ec.l10n.localize("Notifications are blocked for this site; allow them in the browser's site settings"),
            pushInstall: ec.l10n.localize('Add to Home Screen to get notifications when closed'), pushPageOpen: ec.l10n.localize('While a page is open'),
            pushServerOff: ec.l10n.localize('While a page is open; this server does not send push'), pushClosed: ec.l10n.localize('Also when the app is closed'),
            pushFailed: ec.l10n.localize('Could not turn on notifications on this device'),
            // the drawer's rail toggle
            expandMenu: ec.l10n.localize('Expand the menu'), collapseMenu: ec.l10n.localize('Collapse the menu'),
            // Assist (js/WebrootAssist.js; its other texts are moqui.assist.labels)
            assist: ec.l10n.localize('Assist'), askAssist: ec.l10n.localize('Ask Assist') }
    }
}

// The Quasar language pack of the user's locale, one of those vendored, else the first of the language, else en-US.
function quasarLangOf(localeTag) {
    const quasarLangs = ['en-US', 'de', 'es', 'fr', 'it', 'pt-BR', 'ja', 'zh-CN', 'ru', 'nl']
    const language = localeTag.split('-')[0]
    return quasarLangs.includes(localeTag) ? localeTag : quasarLangs.find((tag) => tag.split('-')[0] === language) || 'en-US'
}

;({ build: build, quasarLangOf: quasarLangOf })

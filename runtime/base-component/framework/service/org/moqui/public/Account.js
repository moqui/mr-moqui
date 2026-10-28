/* Framework-only public identity bridge. CC0 plus patent grant. */
const prefix = 'moqui.security.public.'
const fail = (kind, text) => {
    const e = new Error(text)
    e.moquiErrorKind = kind
    throw e
}
const digest = (s) => ec.user.singleUseCodeHash(String(s))
const now = () => ec.user.timestampMillis(ec.user.nowTimestamp)
const find = (name, p) => ec.entity.find(prefix + name).condition(p)
const create = (name, p) => {
    const v = ec.entity.makeValue(prefix + name)
    for (const [k, x] of Object.entries(p)) v.set(k, x)
    v.create()
    return v
}
function recordAttempt(p) {
    const duration = Number(p.windowMillis),
        bucketId = digest(p.key + ':' + Math.floor(now() / duration)),
        old = find('AccountAttempt', { bucketId }).forUpdate(true).one(),
        count = Number(old?.attemptCount || 0) + 1
    if (old) {
        old.set('attemptCount', count)
        old.update()
    } else
        create('AccountAttempt', {
            bucketId,
            attemptCount: count,
            expiresAt: now() + duration * 2,
        })
    return { allowed: count <= Number(p.limit) }
}
/* Compare digests so the comparison time does not depend on how much of a secret matched. */
const secretMatches = (given, expected) =>
    !!given && !!expected && digest(given) === digest(expected)
function clientRegistered(clientId, clientSecret, siteId) {
    const sites = String(
        System.getProperty('pass_client_' + clientId + '_sites') || '',
    ).split(',')
    return (
        !!clientId &&
        secretMatches(
            clientSecret,
            System.getProperty('pass_client_' + clientId + '_secret'),
        ) &&
        (siteId == null || sites.includes(siteId))
    )
}
/* A registered PASS deployment forwards its browser's address; everyone else is limited by their own. */
function requesterKey() {
    const request = ec.web?.request
    if (!request) return 'service'
    const clientId = request.getHeader('x-pass-client') || '',
        forwarded = request.getHeader('x-pass-client-ip') || ''
    if (
        forwarded &&
        clientRegistered(
            clientId,
            request.getHeader('x-pass-client-secret') || '',
            null,
        )
    )
        return 'client:' + clientId + ':' + forwarded.slice(0, 64)
    return request.remoteAddr || 'service'
}
function throttle(action, address) {
    const key = action + ':ip:' + requesterKey()
    const attempt = (key, limit, windowMillis) =>
        ec.service
            .sync()
            .throwOnError(true)
            .name('org.moqui.public.AccountServices.record#Attempt')
            .parameters({ input: { key, limit, windowMillis } })
            .requireNewTransaction(true)
            .call().result.allowed
    if (
        !attempt(key, action === 'verification' ? 20 : 10, 600000) ||
        (address &&
            !attempt(
                action + ':address:' + String(address).trim().toLowerCase(),
                3,
                3600000,
            ))
    )
        fail('Tarpit', 'Wait before requesting more account instructions')
}
function verified() {
    const ua = ec.user.userAccount
    return (
        !!ua &&
        !!find('VerifiedAddress', {
            userId: ua.userId,
            emailAddress: ua.emailAddress,
        }).one()
    )
}
/* Placeholder until verification: nobody, including whoever asked for the mail, knows it. */
function unusablePassword() {
    const rules = ec.user.passwordRules
    return (
        ec.user.randomString(48) +
        '7'.repeat(Number(rules.minDigits) || 0) +
        '!'.repeat(Number(rules.minOthers) || 0)
    )
}
function passwordAllowed(password) {
    const rules = ec.user.passwordRules
    return !(
        password.length < Number(rules.minLength) ||
        (password.match(/\p{Nd}/gu) || []).length < Number(rules.minDigits) ||
        (password.match(/[^\p{Nd}\p{L}]/gu) || []).length <
            Number(rules.minOthers)
    )
}
function beginRegistration(p) {
    if (!/^[^@\s]+@[^@\s]+\.[^@\s]+$/.test(p.emailAddress || ''))
        fail('Unprocessable', 'A valid email address is required')
    const email = p.emailAddress.trim().toLowerCase(),
        template = System.getProperty('pass_verification_template')
    if (!template) fail('Unavailable', 'Verification mail is not configured')
    const old = ec.entity
        .find('moqui.security.UserAccount')
        .condition('emailAddress', email)
        .one()
    const own = old && old.userId === ec.user.userId
    if (ec.user.userId && !own)
        fail('Unprocessable', 'Sign out before registering another address')
    // A disabled account that never verified its address belongs to nobody yet; a later request replaces its challenge.
    const unclaimed =
        old &&
        !own &&
        old.disabled === 'Y' &&
        !find('VerifiedAddress', { userId: old.userId }).one() &&
        !!find('RegistrationChallenge', {
            userId: old.userId,
            newAccount: 'Y',
        }).one()
    if (!old || own || unclaimed) {
        let userId = old?.userId
        if (!old) {
            const placeholder = unusablePassword()
            userId = ec.service
                .sync()
                .name('org.moqui.impl.UserServices.create#UserAccount')
                .parameters({
                    username: email,
                    emailAddress: email,
                    newPassword: placeholder,
                    newPasswordVerify: placeholder,
                    disabled: 'Y',
                    deferIdentityLink: true,
                })
                .call().userId
            if (ec.message.hasError() || !userId) {
                ec.message.clearErrors()
                fail('Unavailable', 'Registration is temporarily unavailable')
            }
        }
        for (const earlier of find('RegistrationChallenge', { userId })
            .forUpdate(true)
            .list())
            if (!earlier.consumedAt) {
                earlier.set('consumedAt', ec.user.nowTimestamp)
                earlier.update()
            }
        const token = ec.user.randomString(64)
        create('RegistrationChallenge', {
            challengeId: ec.user.randomString(32),
            userId,
            emailAddress: email,
            tokenHash: digest(token),
            expiresAt: now() + 3600000,
            newAccount: own ? 'N' : 'Y',
        })
        ec.service
            .sync()
            .name('org.moqui.impl.EmailServices.send#EmailTemplate')
            .parameters({
                emailTemplateId: template,
                toAddresses: email,
                bodyParameters: { verificationToken: token },
            })
            .call()
        // The answer stays the same either way; the challenge remains, so asking again resends.
        if (ec.message.hasError()) {
            ec.message.clearErrors()
            ec.logger.error('Registration verification mail failed for user ' + userId)
        }
    }
    return {
        message:
            'If eligible, verification instructions will be sent to the address.',
    }
}
function verifyRegistration(p) {
    const c = find('RegistrationChallenge', {
        tokenHash: digest(p.token || ''),
    })
        .forUpdate(true)
        .one()
    if (!c || c.consumedAt || ec.user.timestampMillis(c.expiresAt) <= now())
        fail('NotFound', 'Verification link expired or not found')
    if (c.newAccount === 'Y' && !p.newPassword)
        return { passwordRequired: true }
    const ua = ec.entity
        .find('moqui.security.UserAccount')
        .condition('userId', c.userId)
        .forUpdate(true)
        .one()
    if (!ua || ua.emailAddress !== c.emailAddress)
        fail('NotFound', 'Verification link expired or not found')
    if (
        !find('VerifiedAddress', {
            userId: ua.userId,
            emailAddress: ua.emailAddress,
        }).one()
    )
        create('VerifiedAddress', {
            userId: ua.userId,
            emailAddress: ua.emailAddress,
            verifiedAt: ec.user.nowTimestamp,
        })
    if (c.newAccount === 'Y') {
        const password = String(p.newPassword || '')
        if (!passwordAllowed(password))
            fail('Unprocessable', 'Password does not satisfy account policy')
        const out = ec.service
            .sync()
            .name('org.moqui.impl.UserServices.update#PasswordInternal')
            .parameters({
                userId: ua.userId,
                newPassword: password,
                newPasswordVerify: password,
                disabled: 'N',
            })
            .call()
        if (ec.message.hasError() || !out.updateSuccessful) {
            ec.message.clearErrors()
            fail('Unprocessable', 'Password does not satisfy account policy')
        }
    }
    c.set('consumedAt', ec.user.nowTimestamp)
    c.update()
    return { verified: true }
}
function startSession(p) {
    const out = ec.service
        .sync()
        .name('org.moqui.impl.UserServices.login#UserAccount')
        .parameters({
            username: p.username,
            password: p.password,
            code: p.code,
        })
        .call()
    if (!out.loggedIn) {
        if (p.code) {
            ec.message.clearErrors()
            fail('AuthenticationRequired', 'Sign-in failed')
        }
        if (ec.web.sessionAttributes.moquiAuthcFactorRequired) {
            ec.message.clearErrors()
            return { authenticated: false, secondFactorRequired: true }
        }
        ec.message.clearErrors()
        fail('AuthenticationRequired', 'Sign-in failed')
    }
    return { authenticated: true, verified: verified() }
}
function registeredClient(p) {
    if (!clientRegistered(p.clientId, p.clientSecret, p.siteId))
        fail('Unauthorized', 'Unregistered deployment or site')
}
function issueDelegation(p) {
    const ua = ec.user.userId
        ? ec.entity
              .find('moqui.security.UserAccount')
              .condition('userId', ec.user.userId)
              .useCache(false)
              .one()
        : null
    if (!ua) fail('AuthenticationRequired', 'Complete authentication first')
    registeredClient(p)
    const token = ec.user.randomString(64),
        expiresAt = now() + 15 * 60000
    create('DelegatedSession', {
        delegationId: ec.user.randomString(32),
        userId: ua.userId,
        clientId: p.clientId,
        siteId: p.siteId,
        visitId: ec.user.visitId,
        accountStamp: digest(
            String(ua.currentPassword || '') +
                '|' +
                String(ua.passwordSalt || ''),
        ),
        tokenHash: digest(token),
        issuedAt: ec.user.nowTimestamp,
        expiresAt,
    })
    return {
        credential: token,
        expiresAt,
        subject: { handle: ua.username, verified: verified() },
    }
}
function authenticateDelegation(token, clientId, siteId) {
    const v = find('DelegatedSession', {
        tokenHash: digest(token),
        clientId,
        siteId,
    }).one()
    if (!v || v.revokedAt || ec.user.timestampMillis(v.expiresAt) <= now())
        fail('AuthenticationRequired', 'Session expired')
    const ua = ec.entity
        .find('moqui.security.UserAccount')
        .condition('userId', v.userId)
        .one()
    if (
        !ua ||
        ua.disabled === 'Y' ||
        (ua.terminateDate && ec.user.timestampMillis(ua.terminateDate) <= now())
    )
        fail('AuthenticationRequired', 'Session expired')
    if (
        v.accountStamp &&
        v.accountStamp !==
            digest(
                String(ua.currentPassword || '') +
                    '|' +
                    String(ua.passwordSalt || ''),
            )
    ) {
        ec.logger.warn(
            'Delegation rejected because account credentials changed',
        )
        fail('AuthenticationRequired', 'Session expired')
    }
    if (v.visitId) {
        const visit = ec.entity
            .find('moqui.server.Visit')
            .condition('visitId', v.visitId)
            .one()
        if (!visit || visit.thruDate) {
            ec.logger.warn(
                'Delegation rejected because the issuing visit ended or is unavailable',
            )
            fail('AuthenticationRequired', 'Session expired')
        }
    }
    // Activity in the last third of the lifetime slides the expiry, never past twelve hours from issue.
    const expires = ec.user.timestampMillis(v.expiresAt),
        cap = v.issuedAt
            ? ec.user.timestampMillis(v.issuedAt) + 12 * 3600000
            : expires
    if (expires - now() < 5 * 60000 && expires < cap) {
        const renewed = ec.entity
            .find(prefix + 'DelegatedSession')
            .condition('delegationId', v.delegationId)
            .forUpdate(true)
            .one()
        renewed.set('expiresAt', Math.min(now() + 15 * 60000, cap))
        renewed.update()
    }
    // Token issuance followed password/second factor; this request cannot supply a different subject.
    ec.user.internalLoginUser(ua.username)
    return v
}
function ssoHandoff(p, lock) {
    const q = find('SsoHandoff', { handoffId: p.handoffId }),
        row = (lock ? q.forUpdate(true) : q).one()
    if (
        !row ||
        row.consumedAt ||
        ec.user.timestampMillis(row.expiresAt) <= now() ||
        row.siteId !== p.siteId
    )
        fail('NotFound', 'Sign-in handoff expired')
    return row
}
function prepareSso(p) {
    registeredClient(p)
    const callback = System.getProperty(
        'pass_client_' + p.clientId + '_callback',
    )
    if (
        !callback ||
        !/^https?:\/\/[^/]+\//.test(callback) ||
        /[\r\n#]/.test(callback)
    )
        fail('Unavailable', 'SSO callback is not registered')
    if (
        !String(System.getProperty('pass_sso_flows') || '')
            .split(',')
            .includes(p.authFlowId)
    )
        fail('Unauthorized', 'Sign-in provider is unavailable')
    if (
        !/^[A-Za-z0-9_-]{16,80}$/.test(p.state || '') ||
        !/^[A-Za-z0-9_-]{64}$/.test(p.verifier || '')
    )
        fail('Unprocessable', 'Invalid sign-in challenge')
    const flow = ec.entity
        .find('moqui.security.sso.AuthFlow')
        .condition('authFlowId', p.authFlowId)
        .one()
    if (
        !flow ||
        flow.disabled === 'Y' ||
        flow.inbound === 'Y' ||
        !['AftOidc', 'AftOauth'].includes(flow.authFlowTypeEnumId)
    )
        fail('Unavailable', 'Sign-in provider is unavailable')
    const row = create('SsoHandoff', {
        handoffId: ec.user.randomString(32),
        clientId: p.clientId,
        siteId: p.siteId,
        authFlowId: p.authFlowId,
        callbackUri: callback,
        state: p.state,
        verifierHash: digest(p.verifier),
        expiresAt: now() + 600000,
    })
    return { handoffId: row.handoffId }
}
function startSso(p) {
    const row = ssoHandoff(p, false)
    if (row.codeHash) fail('NotFound', 'Sign-in handoff expired')
    ec.web.sessionAttributes.passSsoHandoffId = row.handoffId
    const returnTo =
        '/rest/s1/pass/v1/sites/' +
        encodeURIComponent(row.siteId) +
        '/account/sso-complete?handoffId=' +
        encodeURIComponent(row.handoffId)
    ec.service
        .sync()
        .name('org.moqui.sso.AuthServices.login#User')
        .parameters({ authFlowId: row.authFlowId, returnTo })
        .call()
    return { started: true }
}
function completeSso(p) {
    const row = ssoHandoff(p, true)
    if (
        ec.web.sessionAttributes.passSsoHandoffId !== row.handoffId ||
        row.codeHash
    )
        fail('NotFound', 'Sign-in handoff expired')
    const ua = ec.user.userAccount
    if (!ua)
        fail('AuthenticationRequired', 'Complete account authentication first')
    if (
        ua.disabled === 'Y' ||
        (ua.terminateDate && ec.user.timestampMillis(ua.terminateDate) <= now())
    )
        fail('AuthenticationRequired', 'Account is unavailable')
    const code = ec.user.randomString(64)
    row.set('userId', ua.userId)
    row.set('codeHash', digest(code))
    row.set('codeExpiresAt', now() + 60000)
    row.update()
    ec.web.sessionAttributes.remove('passSsoHandoffId')
    return {
        redirectUrl:
            row.callbackUri +
            '?handoffId=' +
            encodeURIComponent(row.handoffId) +
            '&state=' +
            encodeURIComponent(row.state) +
            '&code=' +
            encodeURIComponent(code),
    }
}
function exchangeSso(p) {
    registeredClient(p)
    const row = ssoHandoff(p, true)
    if (
        row.clientId !== p.clientId ||
        !row.codeHash ||
        row.codeHash !== digest(p.code || '') ||
        row.verifierHash !== digest(p.verifier || '') ||
        ec.user.timestampMillis(row.codeExpiresAt) <= now()
    )
        fail('AuthenticationRequired', 'Sign-in handoff expired')
    const ua = ec.entity
        .find('moqui.security.UserAccount')
        .condition('userId', row.userId)
        .one()
    if (
        !ua ||
        ua.disabled === 'Y' ||
        (ua.terminateDate && ec.user.timestampMillis(ua.terminateDate) <= now())
    )
        fail('AuthenticationRequired', 'Account is unavailable')
    ec.user.internalLoginUser(ua.username)
    row.set('consumedAt', ec.user.nowTimestamp)
    row.update()
    return { ...issueDelegation(p), authenticated: true, verified: verified() }
}
function execute(op, p) {
    const was = ec.artifactExecution.disableAuthz()
    try {
        if (op === 'recordAttempt') return recordAttempt(p)
        if (op === 'prepareSso') return prepareSso(p)
        if (op === 'startSso') return startSso(p)
        if (op === 'completeSso') return completeSso(p)
        if (op === 'exchangeSso') return exchangeSso(p)
        if (op === 'beginRegistration') {
            throttle('registration', p.emailAddress)
            return beginRegistration(p)
        }
        if (op === 'verifyRegistration') {
            throttle('verification')
            return verifyRegistration(p)
        }
        if (op === 'startSession' || op === 'completeSession')
            return startSession(p)
        if (op === 'issueDelegation') return issueDelegation(p)
        if (op === 'requestRecovery') {
            throttle('recovery', p.emailAddress)
            ec.service
                .sync()
                .name('org.moqui.impl.UserServices.reset#Password')
                .parameters({ username: p.emailAddress })
                .call()
            ec.message.clearErrors()
            return {
                message: 'If eligible, recovery instructions will be sent.',
            }
        }
        if (op === 'manageAccount') {
            const ua = ec.user.userAccount
            if (!ua) fail('AuthenticationRequired', 'Sign in to continue')
            if (p.action === 'password')
                return ec.service
                    .sync()
                    .name('org.moqui.impl.UserServices.update#Password')
                    .parameters({
                        userId: ua.userId,
                        oldPassword: p.oldPassword,
                        newPassword: p.newPassword,
                        newPasswordVerify: p.newPasswordVerify,
                        code: p.code,
                    })
                    .call()
            if (p.action === 'endVisit')
                return ec.service
                    .sync()
                    .name('org.moqui.impl.UserServices.end#UserVisit')
                    .parameters({ visitId: p.visitId })
                    .call()
            fail('Unprocessable', 'Unsupported account security action')
        }
        if (op === 'endSession') {
            ec.user.logoutUser()
            return { authenticated: false }
        }
        fail('NotFound', 'Account action not found')
    } finally {
        if (!was) ec.artifactExecution.enableAuthz()
    }
}
;({ execute, authenticateDelegation, clientRegistered })

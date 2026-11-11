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

// The Terminal screen's JSON transitions (webroot apps/Terminal.xml, plans/Terminal-Fancy.md §5.7): each calls one of
// org.moqui.terminal.TerminalServices with the request's named parameters and answers its out-parameters as JSON, or
// {errors, reasonCode}. The services check the caller themselves (the signed-in user's own sessions), so authorization
// is not asked; without a user it is 401.

function web(p) {
  if (!ec.user.userId) {
    ec.web.sendJsonError(401, 'Authentication required')
    return {}
  }
  const parameters = ec.web.parameters
  const input = {}
  for (const name of Array.from(p.names || [])) {
    const value = parameters[name]
    if (value !== undefined && value !== null && value !== '') input[name] = value
  }
  const out = ec.service.sync().name('org.moqui.terminal.TerminalServices.' + p.verb).parameters(input)
    .disableAuthz().call() || {}
  if (ec.message.hasError()) {
    const errors = ec.message.errorsString
    ec.message.clearErrors()
    ec.web.sendJsonResponse(Object.assign({}, out, { errors, reasonCode: out.reasonCode || 'failed' }))
  } else {
    ec.web.sendJsonResponse(out)
  }
  return {}
}

function execute(op, parameters) {
  if (op !== 'web') throw new Error('Unknown terminal operation ' + op)
  return web(parameters)
}

;({ execute })

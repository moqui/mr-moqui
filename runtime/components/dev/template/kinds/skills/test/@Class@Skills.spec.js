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
// The skill pack @Title@: its skill (skill/@package@.md) is in the skill index from its file, and its prompt
// (data/@Class@PromptData.xml) is seeded. Which messages find the skill is `./moq test @component@ --skills`.
// Run: ./moq test @component@

spec({
    setupSpec() { login("john.doe") },
})

feature("the skill is in the index from its file", {
    when() {
        // the skill files are read into the index when a server starts, and again by sync#AgentSkills
        ec.artifactExecution.disableAuthz()
        try {
            ec.service.sync().name("org.moqui.agent.AgentAdminServices.sync#AgentSkills").call()
            return { skill: ec.entity.find("moqui.agent.AgentSkill").condition("skillName", "@package@").list()[0] }
        } finally {
            ec.artifactExecution.enableAuthz()
        }
    },
    then({ skill }) {
        skill.sourceLocation == "component://@component@/skill/@package@.md"
        skill.title == "@Title@"
    },
})

feature("the suggested prompt is seeded", {
    expect() {
        const prompt = ec.entity.find("moqui.agent.AgentSuggestedPrompt").condition("promptId", "@ID@_PROMPT_1").disableAuthz().one()
        prompt.agentId == "assist"
        prompt.sendMode == "prefill"
    },
})

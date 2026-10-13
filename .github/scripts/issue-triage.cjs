const TRIAGE_LABEL = "status: triage";

module.exports = async function runIssueTriage({ github, context }) {
  const event = context.payload;
  if (event.issue.pull_request) return;

  let command;
  if (context.eventName === "issue_comment") {
    if (event.action !== "created" || event.comment.user.type === "Bot") return;
    command = event.comment.body.trim().match(/^\/(triaged|duplicate #([1-9]\d*))$/);
    if (!command) return;

    const { data } = await github.rest.repos.getCollaboratorPermissionLevel({
      ...context.repo,
      username: event.comment.user.login,
    });
    if (!["write", "admin"].includes(data.permission)) return;
  } else if (context.eventName !== "issues" || !["opened", "reopened", "closed"].includes(event.action)) {
    return;
  }

  const source = { ...context.repo, issue_number: event.issue.number };
  const { data: issue } = await github.rest.issues.get(source);
  const removeTriage = async () => {
    if (!issue.labels.some(label => label.name === TRIAGE_LABEL)) return;
    try {
      await github.rest.issues.removeLabel({ ...source, name: TRIAGE_LABEL });
    } catch (error) {
      if (error.status !== 404) throw error;
    }
  };

  if (context.eventName === "issues") {
    if (event.action === "closed") {
      if (issue.state === "closed") await removeTriage();
    } else if (issue.state === "open" && !issue.labels.some(label => label.name === TRIAGE_LABEL)) {
      await github.rest.issues.addLabels({ ...source, labels: [TRIAGE_LABEL] });
    }
    return;
  }

  if (command[1] === "triaged" || issue.state === "closed") {
    await removeTriage();
    return;
  }

  const targetNumber = Number(command[2]);
  if (!Number.isSafeInteger(targetNumber) || targetNumber === issue.number) {
    throw new Error("The duplicate target must be a different issue in this repository.");
  }
  const { data: target } = await github.rest.issues.get({ ...context.repo, issue_number: targetNumber });
  if (target.pull_request) throw new Error("The duplicate target must be an issue, not a pull request.");

  await github.rest.issues.update({
    ...source,
    state: "closed",
    state_reason: "duplicate",
    duplicate_issue_id: target.id,
    headers: { "X-GitHub-Api-Version": "2026-03-10" },
  });
  // GITHUB_TOKEN mutations do not trigger the issue-closed workflow.
  await removeTriage();
};

const GITHUB_URL_RE =
  /https?:\/\/github\.com\/([A-Za-z0-9_.-]+)\/([A-Za-z0-9_.-]+)\/(pull|issues)\/(\d+)\b/i;
function validNumber(value) {
  return Number.isSafeInteger(value) && value > 0;
}
function githubUrl(repo, kind, number) {
  return `https://github.com/${repo}/${kind === "pr" ? "pull" : "issues"}/${number}`;
}
export function parseGithubWorkItemUrl(message) {
  const match = GITHUB_URL_RE.exec(message);
  if (!match) return null;
  const number = Number(match[4]);
  if (!validNumber(number)) return null;
  const repo = `${match[1]}/${match[2]}`;
  const kind = match[3].toLowerCase() === "pull" ? "pr" : "issue";
  return { kind, repo, number, url: githubUrl(repo, kind, number) };
}

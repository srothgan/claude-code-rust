#!/usr/bin/env node
import fs from "node:fs";

const reportPath = process.argv[2] ?? "jscpd-report/jscpd-report.json";
const warningThreshold = Number.parseFloat(process.env.JSCPD_WARNING_THRESHOLD ?? "3.0");

function escapeWorkflowCommand(value) {
  return String(value).replaceAll("%", "%25").replaceAll("\r", "%0D").replaceAll("\n", "%0A");
}

function markdownCell(value) {
  return String(value).replaceAll("|", "\\|").replaceAll("\n", " ");
}

function asNumber(value, fallback = 0) {
  const number = Number(value);
  return Number.isFinite(number) ? number : fallback;
}

function formatPercent(value) {
  return `${asNumber(value).toFixed(2)}%`;
}

function formatLocation(location) {
  if (!location?.name) {
    return "unknown";
  }
  const name = String(location.name).replaceAll("\\", "/");
  const start = asNumber(location.start, asNumber(location.startLoc?.line, 1));
  const end = asNumber(location.end, asNumber(location.endLoc?.line, start));
  return `${name}:${start}-${end}`;
}

function appendSummary(markdown) {
  const summaryPath = process.env.GITHUB_STEP_SUMMARY;
  if (summaryPath) {
    fs.appendFileSync(summaryPath, `${markdown}\n`);
  }
}

function emitWarning(message) {
  console.log(`::warning title=Duplicate code soft threshold::${escapeWorkflowCommand(message)}`);
}

if (!fs.existsSync(reportPath)) {
  const message = `jscpd report not found at ${reportPath}; duplicate-code summary was skipped.`;
  emitWarning(message);
  appendSummary(`### Duplicate Code Scan\n\n${message}\n`);
  process.exit(0);
}

const report = JSON.parse(fs.readFileSync(reportPath, "utf8"));
const total = report.statistics?.total ?? {};
const formats = report.statistics?.formats ?? {};
const duplicates = Array.isArray(report.duplicates) ? report.duplicates : [];

const percentage = asNumber(total.percentage);
const clones = asNumber(total.clones, duplicates.length);
const duplicatedLines = asNumber(total.duplicatedLines);
const totalLines = asNumber(total.lines);
const duplicatedTokens = asNumber(total.duplicatedTokens);
const totalTokens = asNumber(total.tokens);

const status =
  percentage >= warningThreshold
    ? `Warning: duplicated lines are ${formatPercent(percentage)}, above the ${formatPercent(warningThreshold)} soft threshold.`
    : `OK: duplicated lines are ${formatPercent(percentage)}, below the ${formatPercent(warningThreshold)} soft threshold.`;

console.log(status);
console.log(
  `jscpd found ${clones} clones across ${totalLines} lines and ${totalTokens} tokens ` +
    `(${duplicatedLines} duplicated lines, ${duplicatedTokens} duplicated tokens).`,
);

if (percentage >= warningThreshold) {
  emitWarning(
    `jscpd found ${formatPercent(percentage)} duplicated lines (${clones} clones), ` +
      `above the ${formatPercent(warningThreshold)} advisory threshold. ` +
      "This workflow is warning-only and does not block the PR.",
  );
}

const formatRows = Object.entries(formats)
  .sort(([, left], [, right]) => asNumber(right.percentage) - asNumber(left.percentage))
  .map(([format, stats]) =>
    [
      markdownCell(format),
      asNumber(stats.sources),
      asNumber(stats.clones),
      asNumber(stats.duplicatedLines),
      formatPercent(stats.percentage),
    ].join(" | "),
  );

const topDuplicates = [...duplicates]
  .sort((left, right) => asNumber(right.lines) - asNumber(left.lines))
  .slice(0, 10)
  .map((duplicate) =>
    [
      asNumber(duplicate.lines),
      asNumber(duplicate.tokens),
      markdownCell(duplicate.format ?? "unknown"),
      markdownCell(formatLocation(duplicate.firstFile)),
      markdownCell(formatLocation(duplicate.secondFile)),
    ].join(" | "),
  );

const summary = [
  "### Duplicate Code Scan",
  "",
  status,
  "",
  "| Metric | Value |",
  "| --- | ---: |",
  `| Clones | ${clones} |`,
  `| Duplicated lines | ${duplicatedLines} / ${totalLines} (${formatPercent(percentage)}) |`,
  `| Duplicated tokens | ${duplicatedTokens} / ${totalTokens} (${formatPercent(total.percentageTokens)}) |`,
  `| Soft threshold | ${formatPercent(warningThreshold)} |`,
  "",
  "| Format | Files | Clones | Duplicated lines | Duplicated lines % |",
  "| --- | ---: | ---: | ---: | ---: |",
  ...formatRows,
  "",
  "| Lines | Tokens | Format | First location | Second location |",
  "| ---: | ---: | --- | --- | --- |",
  ...(topDuplicates.length > 0 ? topDuplicates : ["| 0 | 0 | none | n/a | n/a |"]),
  "",
  "The complete JSON and HTML reports are attached as the `jscpd-report` workflow artifact.",
  "",
].join("\n");

appendSummary(summary);

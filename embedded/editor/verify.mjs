import assert from "node:assert/strict";
import vm from "node:vm";
import { build } from "esbuild";
import { formatWithCursor } from "prettier/standalone";

export async function verifyFormat(createFormatter, plugins) {
  const cases = [
    ["a.json", "json", '{"emoji":"🙂","items":[1,2]}'],
    ["a.json", "json", '{"bad":}'],
    ["a.js", "babel", "const x={a:1};const view=html`<div><b>hello</b></div>`"],
    ["a.jsx", "babel", "const View=()=> <div title='🙂'>{1+2}</div>"],
    [
      "a.js",
      "babel",
      "const css=String.raw`body{color:red}`; const x=css`a{color:red}`",
    ],
    ["a.ts", "typescript", "const emoji='🙂';const value:{x:number}={x:1}"],
    ["a.tsx", "typescript", "const View=()=> <div title='🙂'>{1+2}</div>"],
    [
      "a.css",
      "css",
      "body{color:red;margin:0 1px}@media(width>1px){a{color:blue}}",
    ],
    [
      "a.html",
      "html",
      "<div><script>const x={a:1}</script><style>a{color:red}</style></div>",
    ],
    [
      "a.md",
      "markdown",
      "# hello\n\n```ts\nconst x={a:1}\n```\n\n|a|b|\n|-|-|\n|1|2|\n",
    ],
    ["a.mdx", "mdx", "import X from './x'\n\n# hello\n\n<X a={1+2}/>\n"],
  ];
  for (const [filepath, parser, source] of cases) {
    for (const cursorOffset of [0, source.indexOf("🙂") + 2, source.length]) {
      let expected = null;
      try {
        expected = JSON.parse(
          JSON.stringify(
            await formatWithCursor(source, {
              filepath,
              parser,
              cursorOffset,
              plugins,
            }),
          ),
        );
      } catch {
        // The native contract returns null for unsupported or malformed input.
      }
      const { formatter, loaded } = createFormatter();
      assert.deepEqual(
        JSON.parse(
          await formatter.vorkspaceFormat(
            filepath,
            source,
            cursorOffset,
            parser,
          ),
        ),
        expected,
        `${filepath} cursor ${cursorOffset}`,
      );
      assert.equal(
        new Set(loaded).size,
        loaded.length,
        "each plugin loads once per runtime",
      );
      if (parser === "json" && expected !== null)
        assert.deepEqual([...loaded].sort(), ["babel", "estree"]);
    }
  }
  console.log(
    `Native formatting matches full-plugin Prettier in ${cases.length * 3} comparisons`,
  );
}

export async function verifyPhp(options, here, analyzer) {
  const bundled = await build({
    ...options,
    entryPoints: undefined,
    outfile: undefined,
    stdin: {
      contents:
        'import { parser } from "@lezer/php"; globalThis.phpParser = parser;',
      resolveDir: here,
    },
    write: false,
  });
  const sandbox = vm.createContext({});
  vm.runInContext(bundled.outputFiles[0].text, sandbox);
  for (const label of ["DOC7", "DOC8", "DOC9", "'DOC8'"]) {
    const end = label.replaceAll("'", "");
    const source = `<?php\n$text = <<<${label}\n${end}more\nbody marker\n${end};\necho $text;\n`;
    const tree = sandbox.phpParser.parse(source);
    const heredocs = [];
    tree.iterate({
      enter(node) {
        assert.equal(node.type.isError, false, `${label} ${tree}`);
        if (node.name === "HeredocString") heredocs.push([node.from, node.to]);
      },
    });
    assert.equal(heredocs.length, 1, label);
    assert.ok(heredocs[0][0] < source.indexOf("body marker"));
    assert.ok(heredocs[0][1] > source.indexOf("body marker") + 11);
    const result = JSON.parse(
      await analyzer.vorkspaceAnalyze("probe.php", source, null),
    );
    assert.ok(
      result.tokens.some(
        (token) =>
          token.kind === "var(--syntax-string)" &&
          token.from <= source.indexOf("body marker") &&
          token.to >= source.indexOf("body marker") + 11,
      ),
      `${label} body is highlighted as a string`,
    );
  }
  for (const [escape, expected] of [
    ["\\x77", "\\x77"],
    ["\\x89", "\\x89"],
    ["\\u{77}", "\\u{77}"],
    ["\\u{89}", "\\u{89}"],
    ["\\7777", "\\777"],
    ["\\89", null],
    ["\\u{}", null],
  ]) {
    const source = `<?php\necho "${escape}";\n`;
    const tree = sandbox.phpParser.parse(source);
    let escapes = 0;
    tree.iterate({
      enter(node) {
        assert.equal(node.type.isError, false, `${escape} ${tree}`);
        if (node.name === "EscapeSequence") {
          assert.equal(source.slice(node.from, node.to), expected);
          escapes++;
        }
      },
    });
    assert.equal(escapes, Number(expected !== null), escape);
  }
  console.log(
    "Native PHP parser accepts decimal heredoc labels and hexadecimal escapes",
  );
}

const fs = require('fs');
const d = require('docx');
const { Paragraph, TextRun, Table, TableRow, TableCell, WidthType, ShadingType, BorderStyle, HeadingLevel, AlignmentType, ImageRun, LevelFormat } = d;

const FONT = 'Arial', MONO = 'Consolas';
const C = { ink: '1A1D21', muted: '5F6B7A', blue: '2563EB', line: 'C9D1DB', soft: 'F3F6FA', amber: 'B45309', amberBg: 'FEF3C7', green: '15803D', greenBg: 'ECFDF5', blueBg: 'E8F0FF' };
const W = 9360; // text width in DXA (US Letter, 1" margins)

// inline markup: **bold**, `code`
function runs(text, base = {}) {
  const out = [];
  const re = /(\*\*[^*]+\*\*|`[^`]+`)/g;
  let last = 0, m;
  while ((m = re.exec(text))) {
    if (m.index > last) out.push(new TextRun({ text: text.slice(last, m.index), font: FONT, ...base }));
    const tok = m[0];
    if (tok.startsWith('**')) out.push(new TextRun({ text: tok.slice(2, -2), bold: true, font: FONT, ...base }));
    else out.push(new TextRun({ text: tok.slice(1, -1), font: MONO, size: (base.size || 22) - 2, color: base.color || '0F3D8A', ...base, font: MONO }));
    last = m.index + tok.length;
  }
  if (last < text.length) out.push(new TextRun({ text: text.slice(last), font: FONT, ...base }));
  return out;
}
const P = (text, o = {}) => new Paragraph({ children: runs(text, { size: o.size || 22, color: o.color || C.ink, italics: o.italics }), spacing: { after: o.after ?? 120, line: 276 }, alignment: o.align, keepNext: o.keepNext });
const H1 = (t) => new Paragraph({ heading: HeadingLevel.HEADING_1, children: [new TextRun({ text: t, font: FONT })] });
const H2 = (t) => new Paragraph({ heading: HeadingLevel.HEADING_2, children: [new TextRun({ text: t, font: FONT })] });
const H3 = (t) => new Paragraph({ heading: HeadingLevel.HEADING_3, children: [new TextRun({ text: t, font: FONT })] });
const bullets = (items, ref = 'bul') => items.map((t) => new Paragraph({ numbering: { reference: ref, level: 0 }, children: runs(t, { size: 22, color: C.ink }), spacing: { after: 70, line: 270 } }));
const numbered = (items, ref) => items.map((t) => new Paragraph({ numbering: { reference: ref, level: 0 }, children: runs(t, { size: 22, color: C.ink }), spacing: { after: 70, line: 270 } }));

const border = (c = C.line, sz = 4) => ({ style: BorderStyle.SINGLE, size: sz, color: c });
const cellBorders = { top: border(), bottom: border(), left: border(), right: border() };
function cell(text, w, o = {}) {
  const paras = (Array.isArray(text) ? text : [text]).map((t) => new Paragraph({ children: runs(String(t), { size: o.size || 19, color: o.color || C.ink, bold: o.bold }), spacing: { after: 40, line: 250 }, alignment: o.align }));
  return new TableCell({ width: { size: w, type: WidthType.DXA }, borders: cellBorders, shading: o.fill ? { type: ShadingType.CLEAR, fill: o.fill, color: 'auto' } : undefined, margins: { top: 70, bottom: 70, left: 100, right: 100 }, children: paras });
}
function table(widths, header, rows, o = {}) {
  const total = widths.reduce((a, b) => a + b, 0);
  const head = new TableRow({ tableHeader: true, cantSplit: true, children: header.map((h, i) => cell(h, widths[i], { bold: true, fill: C.soft, size: 19 })) });
  const body = rows.map((r) => new TableRow({ cantSplit: true, children: r.map((c, i) => cell(c, widths[i], { fill: o.firstColFill && i === 0 ? C.soft : undefined, bold: o.firstColBold && i === 0 })) }));
  return new Table({ width: { size: total, type: WidthType.DXA }, columnWidths: widths, rows: [head, ...body] });
}
const gap = (n = 120) => new Paragraph({ children: [], spacing: { after: n } });

function callout(title, lines, kind = 'blue') {
  const col = { blue: [C.blue, C.blueBg], amber: [C.amber, C.amberBg], green: [C.green, C.greenBg] }[kind];
  const kids = [new Paragraph({ children: [new TextRun({ text: title, bold: true, font: FONT, size: 21, color: col[0] })], spacing: { after: 60 } }), ...lines.map((l) => new Paragraph({ children: runs(l, { size: 21, color: C.ink }), spacing: { after: 60, line: 264 } }))];
  return new Table({
    width: { size: W, type: WidthType.DXA }, columnWidths: [W],
    rows: [new TableRow({ cantSplit: true, children: [new TableCell({ width: { size: W, type: WidthType.DXA }, shading: { type: ShadingType.CLEAR, fill: col[1], color: 'auto' }, borders: { top: border(col[1], 2), bottom: border(col[1], 2), right: border(col[1], 2), left: { style: BorderStyle.SINGLE, size: 24, color: col[0] } }, margins: { top: 120, bottom: 100, left: 200, right: 160 }, children: kids })] })],
  });
}

function code(src) {
  return src.replace(/\t/g, '    ').split('\n').map((line, i, arr) => new Paragraph({
    children: [new TextRun({ text: line === '' ? ' ' : line, font: MONO, size: 16, color: '1F2937' })],
    shading: { type: ShadingType.CLEAR, fill: 'F6F8FA', color: 'auto' },
    spacing: { after: i === arr.length - 1 ? 160 : 0, line: 228 },
    indent: { left: 120, right: 120 },
  }));
}

function figure(file, widthPx, caption) {
  const buf = fs.readFileSync(file);
  // PNG header: width/height at bytes 16..24
  const w = buf.readUInt32BE(16), h = buf.readUInt32BE(20);
  const height = Math.round(widthPx * h / w);
  return [
    new Paragraph({ alignment: AlignmentType.CENTER, children: [new ImageRun({ type: 'png', data: buf, transformation: { width: widthPx, height }, altText: { title: caption, description: caption, name: caption } })], spacing: { before: 80, after: 60 }, keepNext: true }),
    new Paragraph({ alignment: AlignmentType.CENTER, children: [new TextRun({ text: caption, italics: true, font: FONT, size: 18, color: C.muted })], spacing: { after: 180 } }),
  ];
}

// brace-matched extraction of a Rust item from the compiled sketch, so the document shows exactly what was tested
function rustItem(src, startRe) {
  const m = startRe.exec(src);
  if (!m) throw new Error('not found in sketch: ' + startRe);
  let i = src.indexOf('{', m.index), depth = 0;
  for (; i < src.length; i++) { if (src[i] === '{') depth++; else if (src[i] === '}') { depth--; if (depth === 0) break; } }
  const startLine = src.lastIndexOf('\n', m.index) + 1;
  return src.slice(startLine, i + 1);
}
module.exports = { d, C, W, FONT, P, H1, H2, H3, bullets, numbered, table, gap, callout, code, figure, rustItem, runs };

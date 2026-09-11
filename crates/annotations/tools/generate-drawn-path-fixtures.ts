/** Run with: bun crates/annotations/tools/generate-drawn-path-fixtures.ts <local reference checkout>
 * Executes the reference path builder in isolation; no app or installed SDK is needed.
 * The fixture contains generated path coordinates, not vendored SDK implementation.
 */
import { existsSync, readdirSync, readFileSync, writeFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { execFileSync } from 'node:child_process'
const checkout = process.argv[2]
if (!checkout) throw new Error('Pass the path to a local reference checkout.')
const root = resolve(checkout)
const packages = resolve(root, 'packages')
const shapePackage = readdirSync(packages).find(name =>
  existsSync(resolve(packages, name, 'src/lib/shapes/shared/PathBuilder.tsx')))
if (!shapePackage) throw new Error('Reference checkout has no shape path builder.')
const shapes = `packages/${shapePackage}/src/lib/shapes`
const read = (path: string) => readFileSync(resolve(root, path), 'utf8')
const transpiler = new Bun.Transpiler({ loader: 'tsx', target: 'browser' })
const compile = (source: string) => transpiler.transformSync(source.replace(/\bexport /g, ''))
const numbers = read('packages/utils/src/lib/number.ts')
const rngSource = numbers.slice(numbers.indexOf('export function rng'), numbers.indexOf('\n/**', numbers.indexOf('export function rng')))
const modSource = numbers.slice(numbers.indexOf('export function modulate'))
const vecSource = read('packages/editor/src/lib/primitives/Vec.ts').replace(/^import .*\n/gm, '')
const easingSource = read('packages/editor/src/lib/primitives/easings.ts')
const pathSource = read(`${shapes}/shared/PathBuilder.tsx`)
const pathClass = pathSource.slice(pathSource.indexOf('export class PathBuilder {'), pathSource.indexOf('/** @public */\nexport class PathBuilderGeometry2d'))
const cubicSource = pathSource.slice(pathSource.indexOf('const CubicBezier ='))
const stripImports = (s: string) => s.replace(/import[\s\S]*?from [^\n]+\n/g, '')
const arrowSource = stripImports(read(`${shapes}/arrow/ArrowPath.tsx`))
const headsSource = stripImports(read(`${shapes}/arrow/arrowheads.ts`))
const intersectSource = read('packages/editor/src/lib/primitives/intersect.ts')
const intersectStart = intersectSource.indexOf('export function intersectCircleCircle(')
const circleIntersection = intersectSource.slice(intersectStart, intersectSource.indexOf('\n/**', intersectStart))
const prelude = `
const assert = (v, message) => { if (!v) throw Error(message); };
const assertExists = v => { assert(v != null, 'missing value'); return v; };
const approximately = (a,b) => Math.abs(a-b) < 1e-6;
const exhaustiveSwitchError = v => { throw Error(JSON.stringify(v)); };
const toDomPrecision = v => Math.round(v * 1e4) / 1e4;
const getVerticesCountForArcLength = () => 64;
const PI = Math.PI, HALF_PI = Math.PI / 2;
`
// Unused SVG/geometry methods remain in the reference class. Only numerical
// path construction runs, so React, the editor and the browser are unnecessary.
const { PathBuilder, getArrowBodyPathBuilder, getArrowheadPathForType } = new Function(prelude + compile([easingSource, vecSource, rngSource, modSource, pathClass, cubicSource, arrowSource, headsSource, circleIntersection].join('\n')) + '\nreturn {PathBuilder, getArrowBodyPathBuilder, getArrowheadPathForType};')()
const fixtures: any[] = []
for (const tool of ['rectangle', 'ellipse', 'filledRectangle']) {
  for (const [w,h] of [[170,120],[14,20]]) for (const stroke of [4,8]) for (const seed of [7,42]) {
    const path = new PathBuilder()
    if (tool === 'ellipse') path.moveTo(0,h/2).arcTo(w/2,h/2,false,true,0,w,h/2).arcTo(w/2,h/2,false,true,0,0,h/2).close()
    else path.moveTo(0,0).lineTo(w,0).lineTo(w,h).lineTo(0,h).close()
    const options = {strokeWidth:stroke, randomSeed:String(seed)}
    const paths = []
    if (tool === 'filledRectangle') paths.push({filled:true, d:path.toDrawD({...options, passes:1, offset:0})})
    paths.push({filled:false, d:path.toDrawD(options)})
    fixtures.push({tool, w, h, stroke, seed, paths})
  }
}
for (const tool of ['line', 'arrow']) for (const bend of [0, -0.3, 0.5]) for (const stroke of [4,8]) {
  const w = 170, h = 120, seed = 42
  const length = Math.hypot(w,h), sagitta = bend * length
  const cy = bend ? (sagitta*sagitta - length*length/4) / (2*sagitta) : 0
  const center = {x:w/2 - h/length*cy, y:h/2 + w/length*cy}
  const radius = Math.hypot(center.x,center.y), sweep = -4*Math.atan(2*bend)
  const arc = {radius, center, length:radius*sweep, largeArcFlag:Math.abs(sweep)>Math.PI?1:0, sweepFlag:sweep>0?1:0}
  const info = {type:bend?'arc':'straight', start:{point:{x:0,y:0}}, end:{point:{x:w,y:h},arrowhead:'arrow'},bodyArc:arc,handleArc:arc}
  // Zero-offset passes are identical; retain one to compare the visible geometry.
  const paths = [{filled:false,d:getArrowBodyPathBuilder(info).toDrawD({strokeWidth:stroke,randomSeed:String(seed),passes:1})}]
  if (tool === 'arrow') paths.push({filled:false,d:getArrowheadPathForType(info,'end',stroke)})
  fixtures.push({tool,w,h,stroke,seed,bend,paths})
}
writeFileSync(new URL('../tests/fixtures/drawn-paths.json', import.meta.url), JSON.stringify({
  source: 'src/lib/shapes/shared/PathBuilder.tsx:toDrawD',
  commit: execFileSync('git',['rev-parse','HEAD'],{cwd:root,encoding:'utf8'}).trim(),
  fixtures
}, null, 2) + '\n')
console.log(`Generated ${fixtures.length} fixtures from the local reference source.`)

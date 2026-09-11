/** Run: bun crates/annotations/tools/generate-ink-fixtures.ts <local reference checkout>
 * Execute the reference mouse-ink pipeline without loading its editor or dependencies.
 */
import { existsSync, readdirSync, readFileSync, writeFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { execFileSync } from 'node:child_process'
const root = resolve(process.argv[2] ?? '')
const packages = resolve(root, 'packages')
const name = readdirSync(packages).find(name => existsSync(resolve(packages, name, 'src/lib/shapes/shared/freehand/svgInk.ts')))
if (!name) throw Error('Pass a reference checkout containing the freehand renderer.')
const shapes = resolve(packages, name, 'src/lib/shapes')
const read = (file: string) => readFileSync(resolve(shapes, file), 'utf8')
const strip = (s: string) => s.replace(/import[\s\S]*?from [^\n]+\n/g, '').replace(/\bexport /g, '')
const transpiler = new Bun.Transpiler({ loader: 'ts', target: 'browser' })
const settings = read('draw/getPath.ts')
const settingsSource = settings.slice(settings.indexOf('const simulatePressureSettings'), settings.indexOf('const realPressureSettings'))
const numbers = readFileSync(resolve(packages, 'utils/src/lib/number.ts'), 'utf8')
const modulateSource = numbers.slice(numbers.indexOf('export function modulate'))
const easingSource = readFileSync(resolve(packages, 'editor/src/lib/primitives/easings.ts'), 'utf8')
const source = [modulateSource, easingSource, settingsSource, ...['core.ts','getStrokeOutlinePoints.ts','fmt.ts','svgInk.ts'].map(f=>read(`shared/freehand/${f}`))].map(strip).join('\n')
const run = new Function('const assert=(v,m)=>{if(!v)throw Error(m)};\n' + transpiler.transformSync(source) + `
return (points, size, last) => {
 const options={...simulatePressureSettings(size),last};
 const d=svgInk(points,options);
 return {d, samples:Array.from({length:pointCount},(_,i)=>({x:pointX[i],y:pointY[i],radius:radii[i]}))};
};`)()
const cases: Record<string, number[][]> = {
 dot:[[30,40]], duplicates:[[30,40],[30,40],[30,40]], short:[[30,40],[31,40]],
 two:[[30,100],[270,100]],
 slow:Array.from({length:151},(_,i)=>[30+i*2,100]),
 fast:Array.from({length:16},(_,i)=>[30+i*20,100]),
 speed_change:[...Array.from({length:41},(_,i)=>[30+i,130]),...Array.from({length:12},(_,i)=>[80+i*18,130]),...Array.from({length:41},(_,i)=>[280+i,130])],
 curve:Array.from({length:61},(_,i)=>[30+i*5,130+60*Math.sin(i/10)]),
 letter_a:[[35,260],[37,250],[42,230],[52,196],[65,155],[82,114],[100,76],[113,50],[124,31],[130,24],[134,23],[137,27],[139,34],[140,45],[140,59],[139,85],[135,116],[130,151],[126,174],[125,188],[125,196],[129,203],[137,207],[144,210]],
 zigzag:[[30,40],[90,130],[140,30],[180,160],[220,60],[275,140],[310,40]],
 loop:Array.from({length:81},(_,i)=>[180+80*Math.cos(i*Math.PI/40),140+90*Math.sin(i*Math.PI/40)]),
 hook:[[30,50],[70,50],[110,50],[150,50],[190,50],[192,51],[194,53],[194,55],[192,57],[188,58],[182,58],[150,58],[110,58],[70,58],[30,58]],
}
const fixtures=[]
for(const [name,coords] of Object.entries(cases)) for(const size of [4,10,16]) for(const last of [false,true]) {
 const points=coords.map(([x,y])=>({x:Math.fround(x),y:Math.fround(y),z:.5}))
 fixtures.push({name,size,last,points,...run(points,size,last)})
}
writeFileSync(new URL('../tests/fixtures/ink-paths.json',import.meta.url),JSON.stringify({source:'src/lib/shapes/shared/freehand/svgInk.ts',commit:execFileSync('git',['rev-parse','HEAD'],{cwd:root,encoding:'utf8'}).trim(),fixtures},null,2)+'\n')
console.log(`Generated ${fixtures.length} mouse-ink fixtures.`)

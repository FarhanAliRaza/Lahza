import sys,re
txt=open(sys.argv[1]).read()
beats=eval(re.search(r"beats (\[.*?\])",txt).group(1))
ons=[(float(a),float(b)) for a,b,c in re.findall(r"^([\d.]+)\t([\d.]+)\t([\d.]+)$",txt,re.M)]
band=open(sys.argv[2]).read().split('\n')
bo=[]
for line in band:
    for m in re.finditer(r"([\d.]+)\(([\d.]+)\)",line): bo.append((float(m.group(1)),line.split()[0]))
ev=[(13,"ball impact + title cut-on"),(48,"bounce 2 contact"),(124,"drop contact"),(141,"hop contact"),(200,"CUT shot2"),(208,"land on left stone"),(249,"hop takeoff"),(261,"hop land / start charge"),(279,"stone->glass pop, launch"),(308,"land big stone"),(319,"re-land"),(381,"CUT shot3"),(386,"impact on polyhedron"),(396,"CUT macro squash"),(409,"CUT wide"),(413,"stone cracks"),(442,"CUT low angle"),(458,"ball hits floor"),(468,"CUT floor high"),(473,"floor impact"),(520,"squash/anticipation"),(538,"CUT pedestal"),(572,"CUT rope wide"),(587,"lands on rope"),(604,"CUT loop close"),(634,"CUT rope wide 2"),(665,"CUT glass cluster"),(734,"enters glass sphere"),(758,"CUT end card"),(777,"stone lands on blob"),(794,"logo on")]
print("frame\tt\tevent\tnearest onset(full)\tdF\tnearest band onset\tdF\tnearest beat\tdF(beat)")
for f,name in ev:
    t=f/30
    o=min(ons,key=lambda x:abs(x[0]-t)); b=min(bo,key=lambda x:abs(x[0]-t)); be=min(beats,key=lambda x:abs(x-t))
    print(f"{f}\t{t:.3f}\t{name}\t{o[0]:.3f}(s{o[1]:.1f})\t{(o[0]-t)*30:+.1f}\t{b[0]:.3f}[{b[1]}]\t{(b[0]-t)*30:+.1f}\t{be:.3f}\t{(be-t)*30:+.1f}")

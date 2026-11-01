import sys,wave,struct,math
w=wave.open(sys.argv[1]);sr=w.getframerate();n=w.getnframes()
d=struct.unpack('<%dh'%n,w.readframes(n))
win=int(sr*0.004)
def rms(a,b): 
    s=d[a:b]; return math.sqrt(sum(x*x for x in s)/max(1,len(s)))+1e-9
for f in map(int,sys.argv[2:]):
    t=f/30; a=int((t-0.25)*sr)
    env=[(a+i*win, rms(a+i*win,a+(i+1)*win)) for i in range(int(0.5*sr/win))]
    # find biggest jump ratio of env vs mean of previous 40ms
    best=None
    for i in range(10,len(env)):
        prev=sum(e for _,e in env[i-10:i])/10
        r=env[i][1]/prev
        if best is None or r>best[0]: best=(r,env[i][0]/sr)
    print(f"frame {f} t={t:.3f}: sharpest HF attack at {best[1]:.3f}s ({(best[1]-t)*30:+.1f} f), x{best[0]:.1f}")

import math
t0=0.2546; p=0.45006
ev=[("disc reveal/type lock",6),("cut line drawing",41),("cut V/C drawing",73),("cut 'is' ring",111),("'is' ring luma peak",115),("is->where roll",130),("match cut BlackLine",147),("cut sphere+orange line",185),("cut orange arc",205),("cut C4D viewport",219),("split-screen in",257),("cut petal burst",295),("cut close-up shapes",328),("cut ring viewport",364),("wipe midpoint",400),("cut light tubes",431),("tubes->cross",467),("'now' in",473),("end card in",511)]
ons=[(0.232,1.0),(1.602,.38),(8.649,.37),(11.796,.37),(12.249,.37),(15.105,.37),(15.848,.47),(9.845,.35),(0.557,.39)]
C=S=0; errs=[]
for name,n in ev:
    t=(n+1)/30; b=(t-t0)/p; k=round(b); ph=b-k
    near_bar=(t-t0)/(4*p)
    errs.append(abs(ph)); C+=math.cos(2*math.pi*ph); S+=math.sin(2*math.pi*ph)
    print(f"{name:26s} n{n:3d} t={t:6.3f} beat {b:6.2f} -> nearest beat {k} ({'DOWNBEAT' if k%4==0 else 'beat '+str(k%4+1)}) err {ph*p*30:+5.1f} fr  | bar pos {near_bar:5.2f}")
N=len(ev); R=math.hypot(C,S)/N
print("mean |phase| (beats)",round(sum(errs)/N,3),"(uniform random=0.25)","Rayleigh R",round(R,3),"z",round(N*R*R,2),"p~",round(math.exp(-N*R*R),3),"mean phase",round(math.atan2(S,C)/(2*math.pi),3))
# half-beat test
C=S=0
for name,n in ev:
    t=(n+1)/30; b=(t-t0)/(p/2); ph=b-round(b); C+=math.cos(2*math.pi*ph); S+=math.sin(2*math.pi*ph)
R=math.hypot(C,S)/N; print("half-beat Rayleigh R",round(R,3),"p~",round(math.exp(-N*R*R),3))

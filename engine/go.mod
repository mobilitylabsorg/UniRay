module uniray/engine

go 1.26.3

require (
	golang.org/x/crypto v0.49.0
	golang.org/x/sys v0.43.0
)

require golang.org/x/exp v0.0.0-20250711185948-6ae5c78190dc // indirect

require (
	github.com/amnezia-vpn/amneziawg-go/v3 v3.1.20260828
	github.com/google/btree v1.1.3 // indirect
	golang.org/x/net v0.52.0 // indirect
	golang.org/x/time v0.15.0 // indirect
	golang.zx2c4.com/wintun v0.0.0-20230126152724-0fa3db229ce2 // indirect
	gvisor.dev/gvisor v0.0.0-20260710194257-2354a1a30e97 // indirect
)

replace gvisor.dev/gvisor => github.com/mobilitylabsorg/gvisor v0.0.0-20261009073017-f8bf70ec5e36

replace github.com/amnezia-vpn/amneziawg-go/v3 => github.com/mobilitylabsorg/amneziawg-go/v3 v3.1.20260829-0.20261009074143-a1759e5071cd

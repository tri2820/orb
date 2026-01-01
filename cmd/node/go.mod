module github.com/tri/orb/cmd/node

go 1.21

require (
	github.com/google/uuid v1.4.0
	github.com/tri/orb v0.0.0
)

replace github.com/tri/orb => ../..

require (
	github.com/fxamacker/cbor/v2 v2.5.0 // indirect
	github.com/x448/float16 v0.8.4 // indirect
)

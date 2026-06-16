// Self-timed markdown render benchmark.
// Usage: md-bench-go <goldmark|blackfriday> <file> <iters>
// Emits: "<engine>\t<ns_per_op>\t<mb_per_s>\t<out_bytes>" on stdout.
package main

import (
	"bytes"
	"fmt"
	"os"
	"strconv"
	"time"

	bf "github.com/russross/blackfriday/v2"
	"github.com/yuin/goldmark"
	gmext "github.com/yuin/goldmark/extension"
)

var md = goldmark.New(goldmark.WithExtensions(gmext.GFM))

func renderGoldmark(src []byte) int {
	var b bytes.Buffer
	if err := md.Convert(src, &b); err != nil {
		panic(err)
	}
	return b.Len()
}

func renderBlackfriday(src []byte) int {
	return len(bf.Run(src))
}

func main() {
	if len(os.Args) < 4 {
		panic("usage: md-bench-go <engine> <file> <iters>")
	}
	engine := os.Args[1]
	src, err := os.ReadFile(os.Args[2])
	if err != nil {
		panic(err)
	}
	iters, _ := strconv.Atoi(os.Args[3])

	var render func([]byte) int
	switch engine {
	case "goldmark":
		render = renderGoldmark
	case "blackfriday":
		render = renderBlackfriday
	default:
		panic("unknown engine: " + engine)
	}

	warm := iters / 5
	if warm < 3 {
		warm = 3
	}
	outBytes := 0
	for i := 0; i < warm; i++ {
		outBytes = render(src)
	}

	start := time.Now()
	sink := 0
	for i := 0; i < iters; i++ {
		sink += render(src)
	}
	elapsed := time.Since(start)
	_ = sink

	nsPerOp := float64(elapsed.Nanoseconds()) / float64(iters)
	mbPerS := float64(len(src)) * float64(iters) / elapsed.Seconds() / 1.0e6
	fmt.Printf("%s\t%.0f\t%.1f\t%d\n", engine, nsPerOp, mbPerS, outBytes)
}

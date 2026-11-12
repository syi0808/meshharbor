CXX ?= g++
CXXFLAGS ?= -O2 -std=c++17 -Wall -Wextra -pedantic

.PHONY: all test clean
all: glb_refine

glb_refine: src/refine.cpp
	$(CXX) $(CXXFLAGS) $< -o $@

test: glb_refine
	./glb_refine --self-test
	$(CXX) $(CXXFLAGS) tests/test_envelope.cpp -o tests/test_envelope
	./tests/test_envelope
	python3 tests/test_glb_pipeline.py

clean:
	rm -f glb_refine tests/test_envelope
	find tests tools -type d -name __pycache__ -prune -exec rm -rf {} +

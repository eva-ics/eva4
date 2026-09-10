MO_FILES = $(patsubst %.po,%.mo,$(wildcard test_cases/*.po))

%.mo: %.po
	msgfmt -o $@ $<

all: test_cases

test_cases: $(MO_FILES)

clean:
	rm -f test_cases/*.mo

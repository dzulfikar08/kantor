"""Made-up monthly sales for the Kopi Senja coffee chain (8 cities, 2026), as a CSV export.
    python3 make_csv.py OUT.csv"""
import csv, random, sys
random.seed(7)
cities = {"Jakarta": (140, 1.012), "Bandung": (58, 1.01), "Surabaya": (80, 1.0), "Yogyakarta": (50, 1.015),
          "Semarang": (40, 1.01), "Medan": (40, 1.004), "Makassar": (31, 1.02), "Denpasar": (44, 1.018)}
with open(sys.argv[1], "w", newline="") as f:
    w = csv.writer(f)
    w.writerow(["month", "city", "sales_rp_million", "cups"])
    for i in range(12):
        for c, (base, g) in cities.items():
            v = base * g**i * random.uniform(0.92, 1.08)
            w.writerow(["2026-%02d" % (i + 1), c, round(v, 1), int(v * 1e6 / 32000)])

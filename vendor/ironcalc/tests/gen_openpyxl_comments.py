"""Writes tests/openpyxl_comments.xlsx: openpyxl stores the sheet's comments
relationship with an absolute target (/xl/comments/comment1.xml).

Run: uv run --with openpyxl python tests/gen_openpyxl_comments.py
"""

from pathlib import Path

from openpyxl import Workbook
from openpyxl.comments import Comment

wb = Workbook()
ws = wb.active
ws.title = "Sales"
ws["A1"] = "City"
ws["B1"] = "Q1"
ws["A2"] = "Bandung"
ws["B2"] = 420
ws["B2"].comment = Comment("Includes the pop-up stall.", "Rina")
ws["A3"] = "Malang"
ws["A3"].comment = Comment("New shop opened in March.", "Bayu")
wb.save(Path(__file__).with_name("openpyxl_comments.xlsx"))

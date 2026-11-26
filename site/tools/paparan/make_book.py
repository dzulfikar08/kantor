import csv, json, sys
ID = len(sys.argv) > 3 and sys.argv[3] == "id"
T = (lambda en, id_: id_ if ID else en)
rows=list(csv.reader(open(sys.argv[1])))
data=[[T("Month","Bulan"),T("City","Kota"),T("Sales (Rp million)","Penjualan (Rp juta)"),T("Cups","Cangkir")]]
for m,c,s,cups in rows[1:]:
    data.append(["'"+m,c,float(s),int(cups)])
n=len(data)
cities=["Jakarta","Bandung","Surabaya","Yogyakarta","Semarang","Medan","Makassar","Denpasar"]
cols="BCDEFGHI"
summ=[[T("Month","Bulan")]+cities+[T("Total","Total")]]
for i in range(12):
    r=5+i
    line=[f"'2026-{i+1:02d}"]
    for j,c in enumerate(cities):
        col=cols[j]
        line.append(f"=SUMIFS(Data!$C$2:$C${n},Data!$A$2:$A${n},$A{r},Data!$B$2:$B${n},{col}$4)")
    line.append(f"=SUM(B{r}:I{r})")
    summ.append(line)
tot=[T("Total","Total")]+[f"=SUM({c}5:{c}16)" for c in cols+"J"]
summ.append(tot)
book={"sheets":[
 {"name":T("Summary","Ringkasan"),"start":"A4","rows":summ,
  "cells":{"A1":T("Kopi Senja: monthly sales by city, 2026","Kopi Senja: penjualan bulanan per kota, 2026"),"A2":T("Rp million, from penjualan-2026.csv (sheet Data)","Rp juta, dari penjualan-2026.csv (sheet Data)")},
  "formats":[
   {"range":"A1","bold":True,"size":15},
   {"range":"A4:J17","size":10},
   {"range":"A2","italic":True,"font_color":"#595959"},
   {"range":"A4:J4","bold":True,"fill":"#1F4E78","font_color":"#FFFFFF","align":"center"},
   
   {"range":"B5:J17","number_format":"#,##0.0"},
   {"range":"J4:J17","bold":True},
   {"range":"A19:J37","fill":"#FFFFFF"},
   {"range":"A17:J17","bold":True,"borders":{"edges":"top","style":"medium","color":"#1F4E78"}}],
  "widths":{"A":9,"B:D":9,"E":10,"F:I":9,"J":10},
  "freeze":"A5",
  "charts":[{"chart":"line","categories":T("Summary","Ringkasan")+"!A5:A16","series":[{"name":T("Total","Total"),"values":T("Summary","Ringkasan")+"!J5:J16","color":"#1F4E78"}],
     "at":"A19","to":"J37","title":T("Total sales per month (Rp million)","Total penjualan per bulan (Rp juta)"),"legend":"none","number_format":"#,##0"}],
  "page_setup":{"landscape":True,"fit":"page"}},
 {"name":"Data","rows":data,
  "formats":[{"range":"C2:C%d"%n,"number_format":"#,##0.0"},{"range":"D2:D%d"%n,"number_format":"#,##0"}],
  "widths":{"A":11,"B":12,"C":18,"D":9},"freeze":"A2",
  "tables":[{"range":"A1:D%d"%n,"name":T("Sales","Penjualan"),"style":"TableStyleMedium2"}]}],
 "active":T("Summary","Ringkasan")}
json.dump(book,open(sys.argv[2],"w"),indent=1)

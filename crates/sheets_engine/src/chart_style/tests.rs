use super::*;

/// An Excel-like column chart with things Kantor must keep: an extension,
/// a manual layout, an element it doesn't know, text formatting.
const BAR: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:c16r2="http://schemas.microsoft.com/office/drawing/2015/06/chart"><c:date1904 val="0"/><c:chart><c:title><c:tx><c:rich><a:bodyPr rot="0"/><a:lstStyle/><a:p><a:pPr><a:defRPr sz="1600" b="1"/></a:pPr><a:r><a:rPr lang="en-US" sz="1600"/><a:t>Sales</a:t></a:r></a:p></c:rich></c:tx><c:overlay val="0"/></c:title><c:autoTitleDeleted val="0"/><c:plotArea><c:layout><c:manualLayout><c:layoutTarget val="inner"/><c:xMode val="edge"/><c:yMode val="edge"/><c:x val="0.1"/><c:y val="0.2"/><c:w val="0.7"/><c:h val="0.6"/></c:manualLayout></c:layout><c:barChart><c:barDir val="col"/><c:grouping val="clustered"/><c:varyColors val="0"/><c:ser><c:idx val="0"/><c:order val="0"/><c:tx><c:strRef><c:f>Data!$B$1</c:f><c:strCache><c:ptCount val="1"/><c:pt idx="0"><c:v>Sales</c:v></c:pt></c:strCache></c:strRef></c:tx><c:spPr><a:solidFill><a:srgbClr val="FF0000"/></a:solidFill></c:spPr><c:invertIfNegative val="0"/><c:cat><c:strRef><c:f>Data!$A$2:$A$4</c:f><c:strCache><c:ptCount val="3"/><c:pt idx="0"><c:v>Jan</c:v></c:pt><c:pt idx="1"><c:v>Feb</c:v></c:pt><c:pt idx="2"><c:v>Mar</c:v></c:pt></c:strCache></c:strRef></c:cat><c:val><c:numRef><c:f>Data!$B$2:$B$4</c:f><c:numCache><c:formatCode>General</c:formatCode><c:ptCount val="3"/><c:pt idx="0"><c:v>1</c:v></c:pt><c:pt idx="1"><c:v>2</c:v></c:pt><c:pt idx="2"><c:v>3</c:v></c:pt></c:numCache></c:numRef></c:val><c:extLst><c:ext uri="{C3380CC4-5D6E-409C-BE32-E72D297353CC}"><c16:uniqueId xmlns:c16="http://schemas.microsoft.com/office/drawing/2014/chart" val="{00000000-0001}"/></c:ext></c:extLst></c:ser><c:ser><c:idx val="1"/><c:order val="1"/><c:tx><c:strRef><c:f>Data!$C$1</c:f><c:strCache><c:ptCount val="1"/><c:pt idx="0"><c:v>Costs</c:v></c:pt></c:strCache></c:strRef></c:tx><c:invertIfNegative val="0"/><c:cat><c:strRef><c:f>Data!$A$2:$A$4</c:f></c:strRef></c:cat><c:val><c:numRef><c:f>Data!$C$2:$C$4</c:f></c:numRef></c:val></c:ser><c:gapWidth val="219"/><c:axId val="11"/><c:axId val="22"/></c:barChart><c:catAx><c:axId val="11"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:delete val="0"/><c:axPos val="b"/><c:numFmt formatCode="General" sourceLinked="1"/><c:majorTickMark val="none"/><c:minorTickMark val="none"/><c:tickLblPos val="nextTo"/><c:crossAx val="22"/><c:crosses val="autoZero"/><c:auto val="1"/><c:lblAlgn val="ctr"/><c:lblOffset val="100"/><c:noMultiLvlLbl val="0"/></c:catAx><c:valAx><c:axId val="22"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:delete val="0"/><c:axPos val="l"/><c:majorGridlines/><c:numFmt formatCode="General" sourceLinked="1"/><c:majorTickMark val="none"/><c:minorTickMark val="none"/><c:tickLblPos val="nextTo"/><c:crossAx val="11"/><c:crosses val="autoZero"/><c:crossBetween val="between"/></c:valAx><c:spPr><a:noFill/></c:spPr></c:plotArea><c:legend><c:legendPos val="r"/><c:layout><c:manualLayout><c:x val="0.8"/></c:manualLayout></c:layout><c:overlay val="0"/></c:legend><c:plotVisOnly val="1"/><c:dispBlanksAs val="gap"/><c:extLst><c:ext uri="{56B9EC1D-385E-4148-901F-78D8002777C0}"><c16r3:dataDisplayOptions16 xmlns:c16r3="http://schemas.microsoft.com/office/drawing/2017/03/chart"><c16r3:dispNaAsBlank val="1"/></c16r3:dataDisplayOptions16></c:ext></c:extLst></c:chart><c:txPr><a:bodyPr/><a:lstStyle/><a:p><a:pPr><a:defRPr sz="900"/></a:pPr><a:endParaRPr lang="en-US"/></a:p></c:txPr></c:chartSpace>"#;

fn parses(xml: &str) {
    Document::parse(xml).unwrap_or_else(|e| panic!("{e}\n{xml}"));
}

/// Everything Kantor never touches is still there, byte for byte.
fn keeps_extras(xml: &str) {
    for kept in [
        "<c:date1904 val=\"0\"/>",
        "<c:manualLayout><c:layoutTarget val=\"inner\"/><c:xMode val=\"edge\"/><c:yMode val=\"edge\"/><c:x val=\"0.1\"/><c:y val=\"0.2\"/><c:w val=\"0.7\"/><c:h val=\"0.6\"/></c:manualLayout>",
        "<c16r3:dispNaAsBlank val=\"1\"/>",
        "<c:txPr><a:bodyPr/><a:lstStyle/><a:p><a:pPr><a:defRPr sz=\"900\"/></a:pPr><a:endParaRPr lang=\"en-US\"/></a:p></c:txPr>",
        "val=\"{00000000-0001}\"",
    ] {
        assert!(xml.contains(kept), "lost {kept}\n{xml}");
    }
}

#[test]
fn reads_the_panel_state() {
    let s = read(BAR).unwrap();
    assert_eq!(s.kind, ChartType::Column);
    assert!(!s.fixed);
    assert_eq!(s.title, "Sales");
    assert_eq!(s.legend, LegendPos::Right);
    assert_eq!(
        s.colors,
        [SeriesColor::Rgb([255, 0, 0]), SeriesColor::Automatic]
    );
    assert_eq!(s.names, ["Sales", "Costs"]);
    assert_eq!(s.refs[1][2].as_deref(), Some("Data!$C$2:$C$4"));
    assert!(s.has_axes);
}

#[test]
fn title_keeps_its_formatting() {
    let x = set_title(BAR, "Revenue & more").unwrap();
    parses(&x);
    keeps_extras(&x);
    assert!(x.contains("<a:defRPr sz=\"1600\" b=\"1\"/>"));
    assert!(x.contains("<a:rPr lang=\"en-US\" sz=\"1600\"/><a:t>Revenue &amp; more</a:t>"));
    assert_eq!(read(&x).unwrap().title, "Revenue & more");
    let none = set_title(BAR, "").unwrap();
    parses(&none);
    assert!(!none.contains("<c:title>"));
    assert!(none.contains("<c:autoTitleDeleted val=\"1\"/>"));
    // Back again: a fresh title before autoTitleDeleted.
    let back = set_title(&none, "New").unwrap();
    parses(&back);
    assert!(
        back.contains("</c:title><c:autoTitleDeleted val=\"0\"/>"),
        "{back}"
    );
}

#[test]
fn legend_and_axis_titles() {
    let x = set_legend(BAR, LegendPos::Bottom).unwrap();
    parses(&x);
    assert!(
        x.contains("<c:legend><c:legendPos val=\"b\"/><c:overlay val=\"0\"/></c:legend>"),
        "{x}"
    );
    let x = set_legend(&x, LegendPos::None).unwrap();
    assert!(!x.contains("<c:legend>"));
    let x = set_legend(&x, LegendPos::Left).unwrap();
    assert!(x.contains("</c:plotArea><c:legend><c:legendPos val=\"l\"/>"));
    let x = set_axis_title(BAR, true, "Month").unwrap();
    let x = set_axis_title(&x, false, "USD").unwrap();
    parses(&x);
    keeps_extras(&x);
    assert!(
        x.contains("<c:majorGridlines/><c:title>"),
        "title after gridlines: {x}"
    );
    assert!(x.contains("<a:bodyPr rot=\"-5400000\" vert=\"horz\"/>"));
    let s = read(&x).unwrap();
    assert_eq!((s.x_title.as_str(), s.y_title.as_str()), ("Month", "USD"));
    let x = set_axis_title(&x, true, "").unwrap();
    assert_eq!(read(&x).unwrap().x_title, "");
}

#[test]
fn colours() {
    let x = set_color(BAR, 1, SeriesColor::Rgb([0, 0x70, 0xC0])).unwrap();
    parses(&x);
    keeps_extras(&x);
    // In schema order: spPr after tx, before invertIfNegative.
    assert!(x.contains("</c:tx><c:spPr><a:solidFill><a:srgbClr val=\"0070C0\"/></a:solidFill></c:spPr><c:invertIfNegative"), "{x}");
    let x = set_color(&x, 0, SeriesColor::Automatic).unwrap();
    parses(&x);
    assert_eq!(
        read(&x).unwrap().colors,
        [SeriesColor::Automatic, SeriesColor::Rgb([0, 0x70, 0xC0])]
    );
}

#[test]
fn type_changes_round_trip() {
    for k in ChartType::ALL {
        let x = set_kind(BAR, k).unwrap();
        parses(&x);
        keeps_extras(&x);
        let s = read(&x).unwrap();
        assert_eq!(s.kind, k, "{x}");
        assert_eq!(s.names, ["Sales", "Costs"]);
        assert_eq!(s.refs[0][2].as_deref(), Some("Data!$B$2:$B$4"));
        assert_eq!(s.has_axes, k != ChartType::Pie, "{k:?}");
        if k != ChartType::Pie {
            assert_eq!(s.colors[0], SeriesColor::Rgb([255, 0, 0]), "{k:?}: {x}");
        }
        // And back to columns.
        let back = set_kind(&x, ChartType::Column).unwrap();
        parses(&back);
        let b = read(&back).unwrap();
        assert_eq!(b.kind, ChartType::Column);
        assert!(b.has_axes);
    }
    let sc = set_kind(BAR, ChartType::Scatter).unwrap();
    assert!(sc.contains("<c:xVal>") && sc.contains("<c:yVal>"));
    assert_eq!(sc.matches("<c:valAx>").count(), 2, "{sc}");
    assert!(
        !sc.contains("<c:auto "),
        "catAx-only children dropped: {sc}"
    );
    let bar = set_kind(BAR, ChartType::StackedBar).unwrap();
    assert!(bar.contains("<c:barDir val=\"bar\"/><c:grouping val=\"stacked\"/>"));
    assert!(bar.contains("<c:overlap val=\"100\"/>"));
    assert!(bar.contains("<c:axPos val=\"l\"/>") && bar.contains("<c:axPos val=\"b\"/>"));
}

#[test]
fn combo_charts_are_fixed() {
    let combo = BAR.replace(
        "<c:gapWidth val=\"219\"/><c:axId val=\"11\"/><c:axId val=\"22\"/></c:barChart>",
        "<c:gapWidth val=\"219\"/><c:axId val=\"11\"/><c:axId val=\"22\"/></c:barChart><c:lineChart><c:grouping val=\"standard\"/><c:varyColors val=\"0\"/><c:axId val=\"11\"/><c:axId val=\"22\"/></c:lineChart>",
    );
    let s = read(&combo).unwrap();
    assert!(s.fixed);
    assert_eq!(s.kind, ChartType::Other);
    assert!(set_kind(&combo, ChartType::Line).is_err());
    // Titles and colours still work.
    parses(&set_title(&combo, "x").unwrap());
    parses(&set_color(&combo, 0, SeriesColor::Rgb([1, 2, 3])).unwrap());
}

#[test]
fn series_replaced_keeping_formatting() {
    let new = |n: u32| NewSeries {
        tx: Some(format!("<c:strRef><c:f>Data!$A${n}</c:f></c:strRef>")),
        cat: Some("<c:strRef><c:f>Data!$B$1:$D$1</c:f></c:strRef>".into()),
        val: format!("<c:numRef><c:f>Data!$B${n}:$D${n}</c:f></c:numRef>"),
    };
    let x = set_series(BAR, &[new(2), new(3), new(4)]).unwrap();
    parses(&x);
    keeps_extras(&x);
    let s = read(&x).unwrap();
    assert_eq!(s.refs.len(), 3);
    assert_eq!(s.refs[2][2].as_deref(), Some("Data!$B$4:$D$4"));
    assert_eq!(
        s.colors[0],
        SeriesColor::Rgb([255, 0, 0]),
        "first keeps its colour"
    );
    assert!(x.contains("<c:idx val=\"2\"/><c:order val=\"2\"/>"));
    let one = set_series(&x, &[new(2)]).unwrap();
    parses(&one);
    assert_eq!(read(&one).unwrap().refs.len(), 1);
}

#[test]
fn formulas_mapped() {
    let x = map_formulas(BAR, |f| f.replace("Data!", "'My Data'!"));
    assert!(x.contains("<c:f>'My Data'!$B$2:$B$4</c:f>"));
    assert_eq!(
        first_formula("<c:numRef><c:f>'A&amp;B'!$A$1</c:f>"),
        Some("'A&B'!$A$1".into())
    );
}

#[test]
fn default_namespace_prefixes() {
    // A part written without the c: prefix.
    let x = BAR.replace("<c:", "<").replace("</c:", "</").replace(
        "xmlns:c=\"http://schemas.openxmlformats.org/drawingml/2006/chart\"",
        "xmlns=\"http://schemas.openxmlformats.org/drawingml/2006/chart\"",
    );
    let y = set_legend(&x, LegendPos::Top).unwrap();
    parses(&y);
    assert!(y.contains("<legendPos val=\"t\"/>"));
    let y = set_axis_title(&y, true, "Month").unwrap();
    parses(&y);
    assert_eq!(read(&y).unwrap().x_title, "Month");
    let y = set_kind(&y, ChartType::Line).unwrap();
    parses(&y);
    assert_eq!(read(&y).unwrap().kind, ChartType::Line);
}

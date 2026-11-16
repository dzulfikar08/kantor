#!/usr/bin/env python3
"""Generate the region map fixtures (Office 2016 chartex `regionMap`
series, synthetic, CC0-1.0):

- tests/corpus/xlsx/26_region_map.xlsx: one sheet per map (as
  gen_chartex_xlsx.py lays charts out),
- tests/corpus/pptx/region_map.pptx: one map per slide,
- tests/corpus/docx/region_map.docx: the first map, floating.

Maps:
1. "Sales by country": values for ten European countries on a two-colour
   scale, with Czechia in the cache but without a value; value labels.
2. "Trade bloc": a categorical map (text values, `colorStr`) of South
   America, with category name labels.
3. "No map data": values but no geography cache (Office hadn't looked the
   places up): Kantor lists the values.

Office keeps the region outlines Bing returned in
`cx:geography/cx:geoCache`; these fixtures use the documented clear form
(`cx:clear`, [MS-ODRAWXML] 2.24.3.17) with outlines from Natural Earth's
1:110m countries (public domain, https://www.naturalearthdata.com),
simplified (Douglas-Peucker, 0.12 degree), small islands dropped, encoded
with Bing's Point Compression Algorithm as `pcaRings`.

Standard library only. Deterministic: `--check` compares parts.

    python3 scripts/corpus/gen_region_map.py           # write
    python3 scripts/corpus/gen_region_map.py --check   # CI
"""

from __future__ import annotations

import os
import sys
import zipfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import chartex_parts as cxp  # noqa: E402
import gen_chartex_xlsx as gx  # noqa: E402
from gen_shapes_xlsx import HEAD, NS_A, NS_PKG, NS_R, REL, theme  # noqa: E402

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
OUT_XLSX = os.path.join(ROOT, "tests", "corpus", "xlsx", "26_region_map.xlsx")
OUT_PPTX = os.path.join(ROOT, "tests", "corpus", "pptx", "region_map.pptx")
OUT_DOCX = os.path.join(ROOT, "tests", "corpus", "docx", "region_map.docx")
# Region maps need the 2016/5/10 chartex namespace (mc:Choice Requires="cx4").
CX4 = cxp.CX4
BING = "{E9337A44-BEBE-4D9F-B70C-5C5E7DAFC167}"

# Natural Earth 1:110m outlines (public domain), as PCA strings: one per ring.
EUROPE = {
    "France": [
        "sotui_0q4By7_98gK69gismxBkhljysa4n309jNp-8mn2Dgkx24yF0rq16wBt67h8kfh0y6piEjs_82vHg8umwsLur_3v_B4x-msrIhtj_58Hgxhmg6Tku5m2qI8i85pzFtvmwxpoB4q_8629C_7g9zi7BlxsliyJ7u9oj_e9i9trruBo1igqoG03l9kkiDvjr0wxLwjmuznXh9gz434Cjg8qn9tGq80qhlkC6vj5z6Mjt3ss2qBm3lnr7lC_0ropjnB16q8yjjBwukp29zFws-rorX9hso2yV6w6h_yE9nk2iqEypwv8ltCry-0nwG-nnzxlyB9uzql5B",
        "v7ygx6ogwBvzhwozTs4jtp1Tpiop60W-ls64hI05-1xnPszgo-jG",
    ],
    "Poland": [
        "1v-0684_sDq6k0j-rB9rigkhMk1v44i-Diqp5z5Cokhm2-G4o4ols4Bry2hnqJy8rtxrwBxtgw1mc7vtwsiM203iy3Njur2nyF9ot52pVuql1lhMoyo4mjDyyvum7Lxrwr-iD9kksrvKuky18jC217zn-M-rzwy5L2w1plvUno-8vqDt5y45sL31k2x0Jjh0lwxFlhtizoK9-qrv3pMhlyj2rZypvv48B5gk-2lgK3z_vglH6p3nkzF",
    ],
    "Austria": [
        "7x40p5ijtCr22l_rEsq8yr_Fnsv6skD7rz1xmhBi8xtniPuwi99_J5s801j9DktsxviG-ho18kZzmt0-3Bulhqr1kCni0pyrFugt1_rCxn0njuLpismtiD6t5in-3Bnj4xp0Tjlq5ljOj7qt9sEmplgzqMro3o6jV5w2-stkB94w-g5Vw0_jkiFtx4x27Hptvm5wE",
    ],
    "Germany": [
        "mkn5-245zCkhtizoK37__vxF21k2x0Js5y45sLzk63vqD1w1plvU-n86v1J32h8xy_E62sh7zSzl-lr34B2un2yqMn9sn9sEilq5ljOoj4xp0Tyo0tm-3BlrthtiDyn0njuLn8xtlkxBqk-ztqDm2ziq2N4j-_9jN0g2xysa79gismxBz7_98gK86-__xE_nso9rD0901pzL_joymxb33v7s3G_pvwvsH2y2w1oZz563pkF1jwr4iDn7_0q4O3rj2niDi8qk4yZrkkigybtygi21Oihzn-1LgtskrhDpjz_35ckwz60vCzygjjytC61j6m5jD",
    ],
    "Switzerland": [
        "v06vw9ir7B7vijyrFujxw2yZu-gt2uE9xs3m9Go-juo_N0t-tglHirk485En2r_zhC3l9jyiPsnnknnF7ilh78lB-0m12vH1opgqiEhg1x8kf1rq16wBp9-617yBqmk5tqDouzm6qjB",
    ],
    "Belgium": [
        "2_7ploxg7B8zgspzLj569h1B0txi_qI6y7yv-gB-14tnwGzpwv8ltC_nk2iqE7w6h_yEtjl2v1wEwx3t3ggC",
    ],
    "Netherlands": [
        "i0uk-2zqiCy563pkF_yvi1oZuo5ovsHr9k0s3G9joymxbxx3t3ggC8l35s-Xw2kz0nM2i_8i0L3unsmzmDmu899u7Bvzjug4N",
    ],
    "Portugal": [
        "xxunl5_kvBy2-okxZ54kg0mKxy5k_umBuqk50rLy2p1hgK1ihm_xwBslmi29E0mh_k0Skhko9mQiqgj41H4qwm26Ylqp3smDsu5x3kIkwyt6pI3ok90pHtx2ijwnBxoq6ztFx7h61jHmu_pqhK5g6z65Kt_vov4elu_7j5jB",
    ],
    "Spain": [
        "2t-g2vqjkBmqp3smDosz026Yjqgj41Hswwz9mQ1mh_k0S8ivo29E5535_xwBq_n_hgKvqk50rLlnoz9umB64kg0mK6q26jxZuj-x06K9wv37mNjo0uwg1C992ihrmJ22ihmoxDujr0wxLz3l9kkiD4jlnqoG8i9trruBv904j_etso_7vHzjy2hwwBk4gkhwpBs3gr9sxE09wggnRp8gp1xT3iv3p-Nmsqh5_Qx54wo0oBv7mj7p8C5ko4gr4Bqu03ljG-0t5p3tB_sxvjiW",
    ],
    "Italy": [
        "xn7t2ux57BymsixtK39k4-3B7-tj9kZjtsxviGqll96i1Bk33-m3G-l085nI3uz9xwQg3lyhtZ1sv0htN704jvhNm0u3m2mBu4nr49gBnn7tjykC4rg65zLku-r3gEr8inmpE01iwpo7H-nl0nyBjl9ygxF_wthuuTyhjuu8Tlj37unV83q70oW_4ukw0Hrp5x0nEqx9kzikBi7u-sxEw6p-jxU69ix2uD7g-xw-6Bmn6gwt-Eywo6_hMwu78-7xEo7njpsdximyhwfnigm8w5B9z4m3m0C8qygqzFlu5m2qIhxhmg6T95hm68H5x-msrI_687v_Bht73x3Hsk2qnnFj18uyiPm2r_zhC66q_85E8xx1glHn-juo_Nhx6_m9G2jgz2uE",
        "2-o3zxm9yB8_q7grNxpsgwwYil1g79Dsx8zx4J-k4_845H5txvu_H2550r56C",
        "zth2yvy4sBvg2ljnMqq2w5_fxn-k97nB5wg25-Et1qs9nKlunoj3Hz4-nk_c7inxt9L7917y2G",
    ],
    "Czechia": [
        "njqmimhxvC_rzwy5L117zn-M2pux8jC8kksrvKtxvw-iDxyvum7Lnyo4mjDtql1lhMygxo98Qjk412qPux9yt9Hx-8gmkOmkz-_tCy-s5xg2Clq0ustkBwvuiuizE72sh7zSzv1-yy_E_n86v1J",
    ],
}

SOUTH_AMERICA = {
    "Argentina": [
        "4zx0q-pwrIh3-k1_xCvkgn3njCn6zu32yB4u0095Qw52mh0Sp1q_g8Nki-mi81Bhn8tl_8C",
        "xoust7rrsEl6n8ivmDro74h7Sys9vlzDvpmyqlgBmot9tm0Cr6_5x2Mwj-zv_Uhqr9ptF6440m39C1tq559rCqzpljp7Cts0vm5astng5jMwlkh09Q6ix166Iplim67Qlgx95lZkvknx3gBlxw4_5Hr_vtr9X8o1n-3Rj_oy27Nzm5_luMg3guunc_05gh6lBio6juglC6mxwk3Q938popfrq_nwgUki1zs62Bx43o0-Y9xjvo2c84ulo34Byzzqm79B_ig_v6f5mxpyrO6os8-wiBjih5h7cwk2v2-5J2k-qoxSskor9-Kmlkm69Jvt6g2wJhktplxYrpl7rkb59irz7QpzjkxmHvzn0r1jB_2o_il1B30q5r-Ir3xpypHix4pvrE3my29mIojt6vuKjqv2g7Mlxmg96jBssrqz6G81u_o8jBvn48u_sD47tmvnRn-lt4werikj16Ps2m5nhlB8i6xv3N_ml9mysBv_8296P6htqz0D7qtlxi5Chk43ssyB8uj9uhV41kj07dn6h6novBq890v6gBn57tl9I5oh67nFrjih421Bl7jp00uB2gz11yjB1ro0tyC4jtpom4Blggp27sBj00gh-fjom60yaizi7qgalnu48uzIpk9quwRk3vs1_0Gvm548rGjz8i960D0-z8525D1j1207Qws2gwk0B2hivmsUqs8wm5I-0l761Vm6s15vMq-4tp6xDmu-p-0sN",
    ],
    "Chile": [
        "4zx0q-pwrIgn8tl_8Cji-mi81Bo0y-uiK-7r2stZxv1ytn5Gxoi21o0G_-i0tl0Dr1mvj1rNr85u-1R6xt32_S_wqn9lhB0uzvq6M",
        "72go7vgnqExi5mqwZuvz92zN4pwh13Q8n7_t0e4o-1rh7G14zgq3N0ro0tyC2j3k1yjBp251z0uBqjih421Bl-xz7nFm57tl9I6p3kv6gB7zzmnovBoi2zz7d7uj9uhV901jssyB6qtlxi5C5htqz0D720r96Pm-oqmysB7i6xv3N07-nnhlBqikj16Pm-lt4weok26unR7vn-t_sD71u_o8jBs6_iz6Gkxmg96jBiqv2g7MovoxvuK2my29mIhx4pvrEvm9hypH20q5r-I_50pil1Bjirjr1jBozjkxmHtov-y7Q35tsrkbgktplxYut6g2wJ_shv69J0-7h9-K1k-qoxS7-i-_91Hxsxv6-Ozx0296pCy14khrWu2nv92Dthpy2qnBw8h-sl5H0_4gl2Qgv81viOhg0mrtZqkilry_B598h66jB065_zp_Bl_5lv-8Bl794g5_Bux54vzqC6rvpn8nBoi56mr9DjuusnrQ13kj6rtB2z8r2iLnyhiw5gJh02i4qYqo5j98xDs5k046D1vu317iYhqgm803Bo-njwwXlxqt6kiB9x6u338Bj0ny-0gd6-4lokuGoss47gLmymqprL",
    ],
    "Uruguay": [
        "xoust7rrsEv2o9s0Kr35pt46B--og6-C7mmlmtoFgso_uwd6z8sqzP9ym_6nN__3qn-Uwqrx780Bz_-k1sQp6rwy6HklkprkiBrg-h2sJ3zkznlZ92160r4L",
    ],
    "Brazil": [
        "4gs5oqkkqE_ym_6nN6ki4qzPhso_uwd8mmlmtoFrss75-Cs35pt46Bjwh0s0Kurxkh1sNqz_rq6xD23s-5vMg1l761Vyiwom5Is75mqquC0_9z0uFwt5oo_Ng9wjypqCpiz4s1Qmyljv5Govx74-cw2ji04lB9-7jswN3hxv3gF0plpxoIqn7ult0B7w4p22Nsrq803Mmok_j2Ey3px44Jk82tlunC9y-hi2sB-8qt34Bhx_-p_sB4oxq7qO_ryvpzH-q2m-suBqqnv2_Kw2-zh4eymmzzkpC073xkghCvt_z21oBp28y714C0nzuzkF7_n_jihD3m8w-6uBgmoj3glBg7gt4rP577zovPulipt2J9oy-10H_6po1nZ8ph4wpgBj8mw92Dlrh48qDpvu2u7Oywy5u0Hx90i9pYqnjm8r1F7gnw33Qy1ipp-3Hm4jh_8Jt5x0xlM4n_3i-Jmxovi1M40t9xnDggjgo7I5k9-1yHy7tv_qoCnowi_vI2t5y5ujB59qsg6Bp1z8x2V3_r67mN-wk140Emy00w9sB87sz0tF8x400yS7yk8zpB7m319mQ9ngniukBu3uu4pJws4txuB2vn0jmkCw40tqkFr2718roD8ksg9sKlixkumDsxnt19Fs0nt7-D565-hhG7-hziyEu7prk-I5vmnqsHsslsvgT5j83sqfqjw278Vr1wojuFquh6iliC06iq3poBvyv_4wJm_1qm3Ohxph-_lBoutv-oGm73mr2I7g5uv0F0v9ui7Kry6vok5D12zj25C10mx4l0Fztz4_jJ8it90yJiolwnnuBlyozy-Guq2ogglC3jl41iUouux0qlBlri3x45Ii0lw0vV-p877yJksjm4poBljq1ojyDl0o524rBzv1nnhjD_loj8hrDl3-5k8U-1zu29Ooqy87xInior26pDw1rypsuCty7urvkJsuqurk0Gsx50prB189immT1pxtiumCq10-lx8D9ukzitJ7qgru2qBkwvxomkDjg_rr_a06m6x4cjtg-i_Ms0ynziWw9zu0rnC3nw5lzYyvwzvklBz2_4vwoC5lqpo__Buw3uv6Oklk41zJxuqt6ikCwyt8kidlj5u_r5Eiz3qsk0E7i79q8kBgv881qc",
    ],
    "Bolivia": [
        "1521ypo61D7x0-uwgB0wg1zkFthxt814Cj2-l31oBkk_6jghCxmmzzkpCv2-zh4epqnv2_K_q2m-suBm_r3pzH3oxq7qOgx_-p_sBzsyp34B8y-hi2sBj82tlunC64uo44Jlok_j2Errq803M6w4p22Nq-r6kt0BzplpxoIzmmp3gFq4jsj-Bw7qgn5mBy8p3u8hFh--86tiBn2qqvu2C4yx5g0Cjzi7qganzzr0yak00gh-fmggp27sBg0m0nm4Bptk2p3N5o-1rh7G0nwuu0e5pwh13Qvvz92zNyi5mqwZ551gjv2Bn-ojtie-yyk1_F2vi9o0F3ottr4O__vkxhxB3ukhy2Fjr732gyD",
    ],
    "Peru": [
        "r9_889-jkD8gnw33Qypgg7r1Ft1208pYxwy5u0Hlzxru7Okrh48qD3s4q92D7ph4wpgB96po1nZhg4310Htlipt2J477zovP-6gt4rPg9u03glB73w9_6uBjpn3vhGx976kjKir732gyD7os6x2F_i2wwhxBjxxir4O1vi9o0Fr1190_Fm-ojtie7rol8xzDw6-u7gLmrm_hquBo3-npzE8oqv1kmF018s5r0Kx4_9g1c8klu-5Dgyr7vos4B7nm7u64DmrqvqhLr_9tmxmB2rzqgiTyrnvxgmBv7piwqF98rnpjFikuop8Cpnw3qqNzuqry9Pw92104R2qpm78Nvg59z_oBixv88vvBg0zjmj1C7i8mgnRn19vzhPnk907tCigw-g7iEgq6ls_xBkglrgjOov0wrsMllirhqF5537y4Iushx3rci25u4vzB79tg_9Bvhl356T",
    ],
    "Colombia": [
        "p53tyx5t0Clixog6B3t5y5ujBjto69vIz7tv_qoC4k9-1yH--igo7Iw6q4xnDuwmli1Mgugvi-Js5x0xlMl4jh_8Jysx8n-3Hwhl356T89tg_9Byljj5vzBvshx3rct5rzy4ImlirhqFg_1mrsMlglrgjOhq6ls_xBjgw-g7iEr_vw7tC1t-pompB8-kji1D5vl9hlSjk5o0uImuxsiw4Bmjyi02D939s23Di04qmxFyk62n0G6mkrg9F2t1gwqnC952lp7Gyyx1wgLr6qxnnYy354ynE7qrwr4Smw7_9rQvvrl8gHjhoiy8C0vx30sEm8tr9xMxnhz1qBx1--ozFi60zrinBlzgjigFtp21y_iBg7xl2hU9p0xl7Hy7i1txB0vxn9_Nht396w5Esmyp9mDtytxxkFmi8n_nF7y6rgnM82oz6qxC8nuww21Bk60h3qGnmh902T-o5_zjSi90k00PmylmujjCmgpv0ytB-9vlmygCj6lkzkFk-k38se8w9-l5L21l0sl2Buhw0v5Sqi9_s4avjgu5yf",
    ],
    "Venezuela": [
        "xr53mouivCkixkumDkno38sK-g-47roDo1nnqkF3vn0jmkCwnrqxuBt3uu4pJ8ngniukB6m319mQ6yk8zpB0syo0ySkh6s0tF_91hw9sBjshv40Ej_iw7mNq1z8x2Vwjgu5yfri9_s4auu4gw5S31l0sl2B0o0pm5Lkh4m9sek6lkzkF704ulygCngpv0ytBnylmujjCj90k00Pzv6q0jSomh902Tl60h3qG8wllx21Bkg5s7qxCn-21gnMi3ty4pB76unmnJ7_oq0qLu01w2qFhko6vuTt0_vzyZm0o60yE-j4k_2QtgpznlIr32m3kM3uzhr6yBgxvv29GtrnyuoIn74r5-Umw-87yJo0gn8kbs89t2xC3uujgivCy_5x1sT4to4s2O1ooo0mO378oqpB29x59u1Dil3y4sY17-i0qM0vr3moOzv9wljd1_vn9lR06lmnwFtg9n3qHt1rj2ujB2lqs45Dw9t-qvJsw2v6qbhzm4pqE-myp4xFtgwipsmB",
    ],
    "Ecuador": [
        "xv1uhum4nDm19vzhPvwk7-mR41wplj1Cq4-o8vvB741rz_oB_73768Nos5p04R0uqry9Pqnw3qqNjkuop8Cxr4tpjFw7piwqFg452-vevnj8k_Hjo-imrN0t79mhU_9t2jsEyl2_0yV-kro11Etg8z_q_Buy9rioEyyw_mz-Bluxsiw4Bik5o0uI4vl9hlS0jyoi1D0t-pompB",
    ],
    "Paraguay": [
        "vv6p5ty0vD8-7jswN4u3wz4lBnvx74-c2wxqv5Goiz4s1Q-8wjypqCwtrzo_Nz_9z0uFqwvqwzyF46osvk0B5thr07Q1-z8525Dn_uh_60Dwm548rGl3vs1_0Gqk9quwR_-_31pgHjo0kwu2Ctizt7tiBiwp7v8hFv7qgn5mBp4jsj-B",
    ],
}

ISO = {
    "France": "FR", "Spain": "ES", "Portugal": "PT", "Germany": "DE", "Italy": "IT", "Poland": "PL",
    "Netherlands": "NL", "Belgium": "BE", "Switzerland": "CH", "Austria": "AT", "Czechia": "CZ",
    "Brazil": "BR", "Argentina": "AR", "Chile": "CL", "Peru": "PE", "Colombia": "CO", "Bolivia": "BO",
    "Venezuela": "VE", "Paraguay": "PY", "Uruguay": "UY", "Ecuador": "EC",
}

SALES_ROWS = [
    ["Country", "Sales"],
    ["France", 82], ["Spain", 47], ["Portugal", 12], ["Germany", 95], ["Italy", 61],
    ["Poland", 30], ["Netherlands", 38], ["Belgium", 22], ["Switzerland", 27], ["Austria", 18],
]
BLOC_ROWS = [
    ["Country", "Bloc"],
    ["Brazil", "Mercosur"], ["Argentina", "Mercosur"], ["Paraguay", "Mercosur"], ["Uruguay", "Mercosur"],
    ["Bolivia", "Andean"], ["Peru", "Andean"], ["Colombia", "Andean"], ["Ecuador", "Andean"],
    ["Chile", "Pacific"], ["Venezuela", "Other"],
]
NOCACHE_ROWS = [["Country", "Visitors"], ["Japan", 120], ["Korea", 75], ["Vietnam", 40]]

SAFE = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_-"


def decode(s: str) -> list[tuple[float, float]]:
    """Point Compression Algorithm → (latitude, longitude) pairs."""
    out, x, y, i = [], 0, 0, 0
    while i < len(s):
        n, k = 0, 0
        while True:
            b = SAFE.index(s[i])
            i += 1
            n += (b & 31) << k
            k += 5
            if b < 32:
                break
        d = int(((8 * n + 5) ** 0.5 - 1) / 2)
        while d * (d + 1) // 2 > n:
            d -= 1
        while (d + 1) * (d + 2) // 2 <= n:
            d += 1
        ny = n - d * (d + 1) // 2
        nx = d - ny
        x += (nx >> 1) ^ -(nx & 1)
        y += (ny >> 1) ^ -(ny & 1)
        out.append((y / 1e5, x / 1e5))
    return out


def fmt(v: float) -> str:
    return f"{v:.5f}".rstrip("0").rstrip(".")


def clear(shapes: dict[str, list[str]]) -> str:
    """The `cx:clear` cache Office would keep for these countries."""
    locs, datas, points = "", "", ""
    for name, rings in shapes.items():
        iso = ISO[name]
        pts = [p for r in rings for p in decode(r)]
        lats, lons = [p[0] for p in pts], [p[1] for p in pts]
        lat, lon = fmt((min(lats) + max(lats)) / 2), fmt((min(lons) + max(lons)) / 2)
        locs += (
            f'<cx:geoLocationQueryResult><cx:geoLocationQuery countryRegion="{iso}" entityType="CountryRegion"/>'
            f'<cx:geoLocations><cx:geoLocation latitude="{lat}" longitude="{lon}" entityName="{name}" entityType="CountryRegion">'
            f'<cx:address countryRegion="{name}" isoCountryCode="{iso}"/></cx:geoLocation></cx:geoLocations></cx:geoLocationQueryResult>'
        )
        polys = "".join(
            f'<cx:geoPolygon polygonId="{k + 1}" numPoints="{len(decode(r))}" pcaRings="{r}"/>' for k, r in enumerate(rings)
        )
        datas += (
            f'<cx:geoDataEntityQueryResult><cx:geoDataEntityQuery entityType="CountryRegion" entityId="{iso}"/>'
            f'<cx:geoData entityName="{name}" entityId="{iso}" east="{fmt(max(lons))}" west="{fmt(min(lons))}" '
            f'north="{fmt(max(lats))}" south="{fmt(min(lats))}"><cx:geoPolygons>{polys}</cx:geoPolygons>'
            "<cx:copyrights><cx:copyright>Natural Earth (public domain)</cx:copyright></cx:copyrights></cx:geoData>"
            "</cx:geoDataEntityQueryResult>"
        )
        points += (
            f'<cx:geoDataPointToEntityQueryResult><cx:geoDataPointQuery entityType="CountryRegion" latitude="{lat}" longitude="{lon}"/>'
            f'<cx:geoDataPointToEntityQuery entityType="CountryRegion" entityId="{iso}"/></cx:geoDataPointToEntityQueryResult>'
        )
    return (
        f"<cx:clear><cx:geoLocationQueryResults>{locs}</cx:geoLocationQueryResults>"
        f"<cx:geoDataEntityQueryResults>{datas}</cx:geoDataEntityQueryResults>"
        f"<cx:geoDataPointToEntityQueryResults>{points}</cx:geoDataPointToEntityQueryResults></cx:clear>"
    )


def geography(shapes: dict[str, list[str]] | None, projection: str) -> str:
    cache = f'<cx:geoCache provider="{BING}">{clear(shapes)}</cx:geoCache>' if shapes else ""
    return (
        f'<cx:geography projectionType="{projection}" viewedRegionType="countryRegionList" cultureLanguage="en-US" '
        f'cultureRegion="US" attribution="Powered by Bing">{cache}</cx:geography>'
    )


def data(rows: list[list], categorical: bool) -> str:
    rows = rows[1:]
    n = len(rows)
    cats = cxp.lvl([r[0] for r in rows])
    if categorical:
        vals = f'<cx:strDim type="colorStr"><cx:f>Data!$B$2:$B${n + 1}</cx:f>{cxp.lvl([r[1] for r in rows])}</cx:strDim>'
    else:
        vals = f'<cx:numDim type="colorVal"><cx:f>Data!$B$2:$B${n + 1}</cx:f>{cxp.lvl([r[1] for r in rows], "General")}</cx:numDim>'
    return f'<cx:data id="0"><cx:strDim type="cat"><cx:f>Data!$A$2:$A${n + 1}</cx:f>{cats}</cx:strDim>{vals}</cx:data>'


def labels(cat: bool, val: bool) -> str:
    return (
        f'<cx:dataLabels><cx:visibility seriesName="0" categoryName="{int(cat)}" value="{int(val)}"/></cx:dataLabels>'
    )


def sales() -> str:
    colors = (
        f'<cx:valueColors><cx:minColor><a:srgbClr val="DEEBF7"/></cx:minColor>'
        '<cx:maxColor><a:srgbClr val="1F4E79"/></cx:maxColor></cx:valueColors>'
        '<cx:valueColorPositions count="2"/>'
    )
    shapes = {k: v for k, v in EUROPE.items()}
    inner = (
        colors + labels(False, True) + '<cx:dataId val="0"/><cx:layoutPr><cx:regionLabelLayout val="bestFitOnly"/>'
        f"{geography(shapes, 'mercator')}</cx:layoutPr>"
    )
    return cxp.space("Sales by country", data(SALES_ROWS, False), cxp.series("regionMap", "Sales", "Data!$B$1", inner), "", True).replace(
        '<cx:legend pos="t"', '<cx:legend pos="r"'
    )


def bloc() -> str:
    inner = (
        labels(True, False) + '<cx:dataId val="0"/><cx:layoutPr><cx:regionLabelLayout val="bestFitOnly"/>'
        f"{geography(SOUTH_AMERICA, 'miller')}</cx:layoutPr>"
    )
    return cxp.space("Trade bloc", data(BLOC_ROWS, True), cxp.series("regionMap", "Bloc", "Data!$B$1", inner), "", True).replace(
        '<cx:legend pos="t"', '<cx:legend pos="r"'
    )


def nocache() -> str:
    inner = labels(False, True) + f'<cx:dataId val="0"/><cx:layoutPr>{geography(None, "mercator")}</cx:layoutPr>'
    return cxp.space("No map data", data(NOCACHE_ROWS, False), cxp.series("regionMap", "Visitors", "Data!$B$1", inner), "", True)


CHARTS = [
    ("Sales by country", SALES_ROWS, sales, CX4),
    ("Trade bloc", BLOC_ROWS, bloc, CX4),
    ("No map data", NOCACHE_ROWS, nocache, CX4),
]


# ── xlsx: gen_chartex_xlsx's layout with these charts ───────────────────────

def xlsx_parts() -> dict[str, str]:
    saved = cxp.CHARTS
    cxp.CHARTS = CHARTS
    try:
        return gx.parts()
    finally:
        cxp.CHARTS = saved


# ── pptx: a minimal presentation, one map per slide ─────────────────────────

NS_P = "http://schemas.openxmlformats.org/presentationml/2006/main"
NS_MC = "http://schemas.openxmlformats.org/markup-compatibility/2006"
CT_P = "application/vnd.openxmlformats-officedocument.presentationml."


def pptx_parts() -> dict[str, str]:
    n = len(CHARTS)
    hdr = f'xmlns:a="{NS_A}" xmlns:r="{NS_R}" xmlns:p="{NS_P}"'
    tree = (
        '<p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>'
        '<p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/><a:chOff x="0" y="0"/><a:chExt cx="0" cy="0"/></a:xfrm></p:grpSpPr>'
        "{body}</p:spTree></p:cSld>"
    )
    out: dict[str, str] = {}
    ct = (
        HEAD + '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
        '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
        '<Default Extension="xml" ContentType="application/xml"/>'
        f'<Override PartName="/ppt/presentation.xml" ContentType="{CT_P}presentation.main+xml"/>'
        f'<Override PartName="/ppt/slideMasters/slideMaster1.xml" ContentType="{CT_P}slideMaster+xml"/>'
        f'<Override PartName="/ppt/slideLayouts/slideLayout1.xml" ContentType="{CT_P}slideLayout+xml"/>'
        '<Override PartName="/ppt/theme/theme1.xml" ContentType="application/vnd.openxmlformats-officedocument.theme+xml"/>'
    )
    ids, pres_rels = "", ""
    for i, (title, _rows, make, req) in enumerate(CHARTS):
        k = i + 1
        ct += (
            f'<Override PartName="/ppt/slides/slide{k}.xml" ContentType="{CT_P}slide+xml"/>'
            f'<Override PartName="/ppt/charts/chartEx{k}.xml" ContentType="{cxp.CT_CHARTEX}"/>'
        )
        ids += f'<p:sldId id="{255 + k}" r:id="rId{k + 1}"/>'
        pres_rels += f'<Relationship Id="rId{k + 1}" Type="{REL}slide" Target="slides/slide{k}.xml"/>'
        xfrm = '<a:off x="914400" y="1143000"/><a:ext cx="10363200" cy="5257800"/>'
        heading = (
            '<p:sp><p:nvSpPr><p:cNvPr id="2" name="Title 1"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr>'
            '<p:spPr><a:xfrm><a:off x="914400" y="304800"/><a:ext cx="10363200" cy="685800"/></a:xfrm>'
            '<a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr>'
            f'<p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="en-US" sz="2800"/><a:t>{cxp.esc(title)}</a:t></a:r></a:p></p:txBody></p:sp>'
        )
        frame = (
            f'<mc:AlternateContent xmlns:mc="{NS_MC}"><mc:Choice xmlns:cx4="{req}" Requires="cx4">'
            '<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="3" name="Chart 2"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr>'
            f"<p:xfrm>{xfrm}</p:xfrm>"
            f'<a:graphic><a:graphicData uri="{cxp.URI_CHARTEX}"><cx:chart xmlns:cx="{cxp.NS_CX}" r:id="rId2"/></a:graphicData></a:graphic>'
            "</p:graphicFrame></mc:Choice><mc:Fallback>"
            '<p:sp><p:nvSpPr><p:cNvPr id="3" name="Chart 2"/><p:cNvSpPr><a:spLocks noTextEdit="1"/></p:cNvSpPr><p:nvPr/></p:nvSpPr>'
            f'<p:spPr><a:xfrm>{xfrm}</a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr>'
            '<p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="en-US" sz="1100"/>'
            "<a:t>This chart isn't available in your version of PowerPoint.</a:t></a:r></a:p></p:txBody></p:sp>"
            "</mc:Fallback></mc:AlternateContent>"
        )
        out[f"ppt/slides/slide{k}.xml"] = HEAD + f"<p:sld {hdr}>" + tree.format(body=heading + frame) + "</p:sld>"
        out[f"ppt/slides/_rels/slide{k}.xml.rels"] = (
            HEAD + f'<Relationships xmlns="{NS_PKG}">'
            f'<Relationship Id="rId1" Type="{REL}slideLayout" Target="../slideLayouts/slideLayout1.xml"/>'
            f'<Relationship Id="rId2" Type="{cxp.REL_CHARTEX}" Target="../charts/chartEx{k}.xml"/></Relationships>'
        )
        out[f"ppt/charts/chartEx{k}.xml"] = make()
    out["[Content_Types].xml"] = ct + "</Types>"
    out["_rels/.rels"] = (
        HEAD + f'<Relationships xmlns="{NS_PKG}"><Relationship Id="rId1" Type="{REL}officeDocument" Target="ppt/presentation.xml"/></Relationships>'
    )
    out["ppt/presentation.xml"] = (
        HEAD + f'<p:presentation {hdr}><p:sldMasterIdLst><p:sldMasterId id="2147483648" r:id="rId1"/></p:sldMasterIdLst>'
        f'<p:sldIdLst>{ids}</p:sldIdLst><p:sldSz cx="12192000" cy="6858000"/><p:notesSz cx="6858000" cy="9144000"/></p:presentation>'
    )
    out["ppt/_rels/presentation.xml.rels"] = (
        HEAD + f'<Relationships xmlns="{NS_PKG}"><Relationship Id="rId1" Type="{REL}slideMaster" Target="slideMasters/slideMaster1.xml"/>'
        f'{pres_rels}<Relationship Id="rId{n + 2}" Type="{REL}theme" Target="theme/theme1.xml"/></Relationships>'
    )
    clr_map = (
        '<p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" '
        'accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/>'
    )
    out["ppt/slideMasters/slideMaster1.xml"] = (
        HEAD + f"<p:sldMaster {hdr}>" + tree.format(body="") + clr_map
        + '<p:sldLayoutIdLst><p:sldLayoutId id="2147483649" r:id="rId1"/></p:sldLayoutIdLst></p:sldMaster>'
    )
    out["ppt/slideMasters/_rels/slideMaster1.xml.rels"] = (
        HEAD + f'<Relationships xmlns="{NS_PKG}"><Relationship Id="rId1" Type="{REL}slideLayout" Target="../slideLayouts/slideLayout1.xml"/>'
        f'<Relationship Id="rId2" Type="{REL}theme" Target="../theme/theme1.xml"/></Relationships>'
    )
    out["ppt/slideLayouts/slideLayout1.xml"] = (
        HEAD + f'<p:sldLayout {hdr} type="blank">' + tree.format(body="") + "<p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sldLayout>"
    )
    out["ppt/slideLayouts/_rels/slideLayout1.xml.rels"] = (
        HEAD + f'<Relationships xmlns="{NS_PKG}"><Relationship Id="rId1" Type="{REL}slideMaster" Target="../slideMasters/slideMaster1.xml"/></Relationships>'
    )
    out["ppt/theme/theme1.xml"] = theme()
    return out


# ── docx: the first map, floating between two paragraphs ────────────────────

def docx_parts() -> dict[str, str]:
    import gen_docx_chartex as gd

    doc = gd.document().replace(f'xmlns:cx1="{cxp.CX1}" Requires="cx1"', f'xmlns:cx4="{CX4}" Requires="cx4"')
    parts = gd.parts()
    parts["word/document.xml"] = doc
    parts["word/charts/chartEx1.xml"] = sales().replace("Data!", "Sheet1!")
    return parts


def write(path: str, parts: dict[str, str]) -> None:
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as z:
        names = ["[Content_Types].xml"] + sorted(n for n in parts if n != "[Content_Types].xml")
        for name in names:
            info = zipfile.ZipInfo(name, (2026, 11, 16, 9, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            z.writestr(info, parts[name].encode("utf-8"))


OUTPUTS = [(OUT_XLSX, xlsx_parts), (OUT_PPTX, pptx_parts), (OUT_DOCX, docx_parts)]


def main() -> int:
    if "--check" in sys.argv:
        bad = 0
        for path, make in OUTPUTS:
            with zipfile.ZipFile(path) as z:
                have = {n: z.read(n).decode("utf-8") for n in z.namelist()}
            if have != make():
                print(f"{path} is out of date: run {sys.argv[0]}", file=sys.stderr)
                bad = 1
        return bad
    for path, make in OUTPUTS:
        write(path, make())
        print(path)
    return 0


if __name__ == "__main__":
    sys.exit(main())

import XCTest
@testable import SoundroomCore

final class ProfessionalMixingTests: XCTestCase {
    func fixture() -> (Session, [UUID: AudioSamples]) {
        let id = UUID()
        let samples = (0..<24000).map { Float(sin(Double($0) * 0.041)) * 0.2 }
        var session = Session(), clip = Clip(assetID: id, name: "Signal", start: 0, length: samples.count)
        clip.edgeProtectionDisabled = true; session.tracks[0].clips = [clip]
        return (session, [id: AudioSamples(channels: [samples,samples])])
    }
    func render(_ session: Session, _ assets: [UUID: AudioSamples], block: Int = 257, start: Int = 0, length: Int = 24000) -> [Float] {
        let mixer = RealtimeMixer(session: session,assets: assets,startFrame: start)
        var l = [Float](repeating: 0,count: length), r = l
        l.withUnsafeMutableBufferPointer { lp in r.withUnsafeMutableBufferPointer { rp in
            var offset = 0
            while offset < length { let count = min(block,length-offset); mixer.render(intoLeft: lp.baseAddress!+offset,intoRight: rp.baseAddress!+offset,frameCount: count); offset += count }
        } }
        return l
    }
    func testUtilityChannelMatrixAndIntentionalDelay() {
        let input = AudioSamples(channels: [[1,0,0,0],[0.5,0,0,0]])
        let effect = RealtimeEffect(kind: .utility,parameters: ["polarityL":1,"swap":1,"delayL":2,"delayR":1])
        let out = effect.process(input).channels
        XCTAssertEqual(out[0],[0,0,0.5,0]); XCTAssertEqual(out[1],[0,-1,0,0])
        let mono = RealtimeEffect(kind:.utility,parameters:["mono":1,"width":2]).process(input).channels
        XCTAssertEqual(mono[0],mono[1]); XCTAssertEqual(mono[0][0],0.75)
        XCTAssertEqual(effect.latencyFrames,0); XCTAssertEqual(effect.tailFrames,2)
    }
    func testGateHoldHysteresisStereoLinkAndUnityExpansion() {
        var v = RealtimeEffectKind.gate.parameters.map(\.defaultValue)
        let state = GateDSP()
        for _ in 0..<24000 { _ = state.process(0.2,0.00001,v) }
        XCTAssertLessThan(state.reductionDB,0.01)
        // Above closing threshold while below opening threshold: stays open.
        for _ in 0..<4800 { _ = state.process(0.008,0.001,v) }
        XCTAssertLessThan(state.reductionDB,0.01)
        for _ in 0..<2000 { _ = state.process(0,0,v) }
        XCTAssertLessThan(state.reductionDB,0.01)
        for _ in 0..<48000 { _ = state.process(0,0,v) }
        XCTAssertGreaterThan(state.reductionDB,55)
        v[1] = 1
        let unity = GateDSP(); XCTAssertEqual(unity.process(0.001,0.0001,v).0,0.001)
    }
    func testAdditionalBellsMatchResponseAndDynamicCutsFollowLevel() {
        let samples = (0..<48000).map { Float(sin(Double($0) * 2 * .pi * 2000 / sessionRate)) * 0.2 }
        let audio = AudioSamples(channels:[samples])
        let off = RealtimeEffect(kind:.equalizer,parameters:["band5Frequency":2000,"band5Gain":6]).process(audio).channels[0]
        XCTAssertEqual(EQResponse.decibels(at:2000,parameters:["band5Frequency":2000,"band5Gain":6]),6,accuracy:0.001)
        let on = RealtimeEffect(kind:.equalizer,parameters:["band5Frequency":2000,"band5Gain":6,"band5Threshold":-30,"band5Range":-12]).process(audio).channels[0]
        func power(_ a:[Float]) -> Float { a.suffix(12000).reduce(0) { $0+$1*$1 } }
        XCTAssertLessThan(power(on),power(off)*0.12)
    }
    func testGroupSendCompensationPartitionSeekAndSolo() throws {
        var (session,assets) = fixture(); let dry = render(session,assets)
        var group = Track(name:"Group",color:2), ret = Track(name:"Return",color:3)
        group.routing = TrackRouting(role:"bus"); group.gain = 0.5
        group.realtimeEffects = [RealtimeEffect(kind:.limiter,enabled:false)]
        ret.routing = TrackRouting(role:"return",outputID:group.id); ret.gain = 0.25
        ret.realtimeEffects = [RealtimeEffect(kind:.limiter,enabled:false)]
        session.tracks[0].routing = TrackRouting(outputID:group.id,sends:[TrackSend(trackID:ret.id,level:0.8)])
        session.tracks += [group,ret]; _ = try TrackRoutingPlan(session:session)
        let out = render(session,assets)
        for i in out.indices { XCTAssertEqual(out[i],dry[i]*0.6,accuracy:0.000002) }
        XCTAssertEqual(out,render(session,assets,block:1024))
        XCTAssertEqual(Array(out[4000..<9000]),render(session,assets,start:4000,length:5000))
        session.tracks[1].solo = true
        XCTAssertEqual(out,render(session,assets))
        XCTAssertEqual(session.requiredProjectVersion,10)
        let decoded = try JSONDecoder().decode(Session.self,from:JSONEncoder().encode(session))
        XCTAssertEqual(decoded.tracks,session.tracks)
    }
    func testSidechainCompensatesEarlierInsertAndDoesNotMixKey() {
        var (session,assets) = fixture(); var key = session.tracks[0], sink = Track(name:"Key sink",color:2)
        key.id = UUID(); key.clips[0].id = UUID(); sink.routing = TrackRouting(role:"bus"); sink.muted = true
        key.routing = TrackRouting(outputID:sink.id)
        session.tracks[0].routing = TrackRouting(sidechainSourceID:key.id)
        session.tracks[0].realtimeEffects = [RealtimeEffect(kind:.compressor,parameters:["threshold":-40,"attack":0.1])]
        session.tracks += [key,sink]; let expected = render(session,assets)
        session.tracks[0].solo = true
        XCTAssertEqual(expected,render(session,assets))
        session.tracks[0].solo = false
        session.tracks[0].realtimeEffects.insert(RealtimeEffect(kind:.limiter,enabled:false),at:0)
        XCTAssertEqual(expected,render(session,assets))
        session.tracks[1].muted = true; let unkeyed = render(session,assets)
        func power(_ a:[Float]) -> Float { a.suffix(12000).reduce(0) { $0+$1*$1 } }
        XCTAssertLessThan(power(expected),power(unkeyed)*0.1)
    }
    func testRoutingRejectsFeedbackAndMissingTracks() throws {
        var (session,_) = fixture(); var group = Track(name:"Group",color:2)
        group.routing = TrackRouting(role:"bus"); session.tracks[0].routing = TrackRouting(outputID:group.id); session.tracks.append(group)
        _ = try TrackRoutingPlan(session:session)
        let sourceID = session.tracks[0].id
        session.tracks[1].routing?.sidechainSourceID = sourceID
        _ = try TrackRoutingPlan(session:session)
        session.tracks[0].routing?.sidechainSourceID = group.id
        XCTAssertThrowsError(try TrackRoutingPlan(session:session))
        session.tracks[0].routing = TrackRouting(outputID:UUID())
        XCTAssertThrowsError(try TrackRoutingPlan(session:session))
    }
}

extension ProfessionalMixingTests {
    func testMasteringProposalMatchesSwiftGoldenIncludingExpandedEQ() throws {
        let root = URL(fileURLWithPath:#filePath).deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        let file = root.appendingPathComponent("portable/crates/engine/tests/fixtures/mastering-swift-reference.json")
        var fixture = try XCTUnwrap(JSONSerialization.jsonObject(with:Data(contentsOf:file)) as? [String:Any])
        let analysis = try JSONDecoder().decode(MasteringAnalysis.self,from:JSONSerialization.data(withJSONObject:fixture["analysis"]!))
        var cases = try XCTUnwrap(fixture["proposals"] as? [[String:Any]])
        let encoder = JSONEncoder(); encoder.outputFormatting = [.sortedKeys]
        let fixedID = UUID(uuidString:"00000000-0000-4000-8000-000000000001")!
        for i in cases.indices {
            let target = try XCTUnwrap(cases[i]["targetLUFS"] as? NSNumber), intensity = try XCTUnwrap(cases[i]["intensity"] as? NSNumber)
            var effects = analysis.proposal(targetLUFS:target.floatValue,intensity:intensity.floatValue)
            for j in effects.indices { effects[j].id = fixedID }
            let text = String(decoding:try encoder.encode(effects),as:UTF8.self).replacingOccurrences(of:fixedID.uuidString,with:"ID")
            if ProcessInfo.processInfo.environment["SOUNDROOM_UPDATE_MIXING_GOLDEN"] == "1" { cases[i]["swiftJSON"] = text }
            else { XCTAssertEqual(text,cases[i]["swiftJSON"] as? String) }
        }
        var fixed = analysis
        fixed.integratedLUFS = -19; fixed.truePeakDB = -3; fixed.bassFraction = 0.71; fixed.airFraction = 0.42; fixed.analyzedFrames = 300000
        fixed.sections = [.init(start:0,end:144000,loudness:-24,truePeak:-6,resonance:2), .init(start:144000,end:288000,loudness:-16,truePeak:-0.5,resonance:0.4), .init(start:288000,end:300000,loudness:-70,truePeak:-40,resonance:3.1)]
        var fixedEffects = fixed.proposal(targetLUFS:-12,intensity:0.8)
        for j in fixedEffects.indices { fixedEffects[j].id = fixedID }
        let fixedText = String(decoding:try encoder.encode(fixedEffects),as:UTF8.self).replacingOccurrences(of:fixedID.uuidString,with:"ID")
        if ProcessInfo.processInfo.environment["SOUNDROOM_UPDATE_MIXING_GOLDEN"] == "1" {
            fixture["fixedProposal"] = fixedText
            fixture["proposals"] = cases
            try JSONSerialization.data(withJSONObject:fixture,options:[.prettyPrinted,.sortedKeys,.withoutEscapingSlashes]).write(to:file,options:.atomic)
        } else { XCTAssertEqual(fixedText,fixture["fixedProposal"] as? String) }
    }
    func testRoutedLiveEditsKeepHistoryAndStemKeepsExternalKey() throws {
        var (session,assets) = fixture(); var key = session.tracks[0], sink = Track(name:"Key sink",color:2)
        key.id = UUID(); key.clips[0].id = UUID(); sink.routing = TrackRouting(role:"bus"); sink.muted = true
        key.routing = TrackRouting(outputID:sink.id)
        session.tracks[0].routing = TrackRouting(sidechainSourceID:key.id)
        session.tracks[0].realtimeEffects = [RealtimeEffect(kind:.compressor,parameters:["threshold":-40,"attack":0.1])]
        session.tracks += [key,sink]
        let expected = render(session,assets)
        let isolated = try session.isolating(trackID:session.tracks[0].id)
        XCTAssertEqual(render(isolated,assets),expected)
        let mixer = RealtimeMixer(session:session,assets:assets)
        var l = [Float](repeating:0,count:4000), r = l
        l.withUnsafeMutableBufferPointer { lp in r.withUnsafeMutableBufferPointer { rp in mixer.render(intoLeft:lp.baseAddress!,intoRight:rp.baseAddress!,frameCount:4000) } }
        session.tracks[0].gain = 0.5
        XCTAssertFalse(RealtimeMixer.requiresAlignedUpdate(from:isolated,to:isolated))
        mixer.update(session:session,assets:assets)
        l.withUnsafeMutableBufferPointer { lp in r.withUnsafeMutableBufferPointer { rp in mixer.render(intoLeft:lp.baseAddress!,intoRight:rp.baseAddress!,frameCount:4000) } }
        for i in l.indices { XCTAssertEqual(l[i],expected[i+4000]*0.5,accuracy:0.000002) }
    }
}

extension ProfessionalMixingTests {
    func testAutomationOnlyExpandedEQRequiresVersionTenAndRoutedMasterMatchesFlatMix() {
        var (session,assets) = fixture()
        var eq = RealtimeEffect(kind:.equalizer,parameters:["lowGain":0])
        eq.parameterAutomation["band4Gain"] = [.init(frame:0,value:0.5)]
        session.masterTrack.realtimeEffects = [eq]
        XCTAssertEqual(session.requiredProjectVersion,10)
        session.masterTrack.realtimeEffects = [RealtimeEffect(kind:.compressor,parameters:["threshold":-28,"attack":0.1])]
        session.masterTrack.mixAutomation["volume"] = [.init(frame:0,value:0.2),.init(frame:24000,value:0.9)]
        let flat = render(session,assets)
        session.tracks[0].routing = TrackRouting()
        let routed = render(session,assets)
        for i in flat.indices { XCTAssertEqual(routed[i],flat[i],accuracy:0.000002) }
    }
}

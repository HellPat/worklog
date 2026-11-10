Feature: Annotate a passage in the launched native fixture

  Scenario: Centered driver timeline has a visible sidebar and independent breakpoint controls
    Given a fresh launched fixture for "driver-timeline"
    When I use the centered timeline and create breakpoints without recording

  Scenario: Bottom aligned conversation has notes breakpoints and contextual span actions
    Given a fresh launched fixture for "conversation-notes"
    When I use the live conversation notes and breakpoint composer
  Scenario Outline: Save a note through the macOS desktop and reopen it - <case>
    Given a fresh launched fixture for "<case>"
    When I start and stop the recording simulation
    And I open and close Settings
    And I drag a range on the "<region>"
    And I enter and save an annotation
    Then the note is displayed and stored in SQLite
    And the note remains displayed after reopening the app

    Examples:
      | case    | region                    |
      | chart-1 | Voice activity chart      |
      | chart-2 | Voice activity chart      |
      | chart-3 | Voice activity chart      |
      | track-1 | Transcript range selector |

  Scenario: Adjust a span with the timeline beside its labeling fields
    Given a fresh launched fixture for "chart-refine"
    When I drag a range on the "Voice activity chart"
    And I adjust the span using the timeline
    And I enter and save an annotation
    Then the note is displayed and stored in SQLite
    And the note remains displayed after reopening the app

  Scenario: Footer starts a draft span and requires a label before saving
    Given a fresh launched fixture for "footer-start"
    When I start and stop the recording simulation
    And I open and close Settings
    And I start and end a span without details
    Then the unnamed span requires a label before saving

  Scenario: Comment on a transcript message and reopen
    Given a fresh launched fixture for "comment"
    When I comment on the first message
    Then the comment survives reopening

  Scenario: Real audio transcript can be labeled without changing its text
    Given a fresh launched fixture for "audio"
    When I label the imported audio transcript
    And I enter and save an annotation
    Then the audio transcript and span are independently stored and linked
    And the note remains displayed after reopening the app

  Scenario: Named running span preserves optional details
    Given a fresh launched fixture for "named-span"
    When I start and end a named span
    Then the running span details are stored

  Scenario: Refinement keeps its timeline and draft through Settings
    Given a fresh launched fixture for "refine-navigation"
    When I label the imported audio transcript
    And I keep my draft through Settings and adjust the timeline
    And I enter and save an annotation
    Then the note is displayed and stored in SQLite
    And the note remains displayed after reopening the app

  Scenario: Sidebar collapses and empty dates clear the transcript
    Given a fresh launched fixture for "sidebar"
    When I collapse and reopen the sidebar and choose an empty date
    Then the selected date contains no transcript messages

  Scenario: Reset removes a marked range and preserves the description draft
    Given a fresh launched fixture for "reset"
    When I reset a marked span
    Then saving the cleared span is disabled

  Scenario: Saved spans and actions follow the selected day
    Given a fresh launched fixture for "span-tree"
    When I browse saved spans across days

  Scenario: Conversation stays visible while a popover labels the selected span
    Given a fresh launched fixture for "conversation-popover"
    When I add a labeled span through the conversation popover

  Scenario: Saving a span keeps the selected day and exact range
    Given a fresh launched fixture for "selected-day-save"
    When I save a span after switching to the next fixture day

  Scenario: Calendar browses previous months and empty days offer recording without Back to live
    Given a fresh launched fixture for "previous-months"
    When I browse previous months and return to today's empty transcript

  @local_audio
  Scenario: Recording streams real fixture speech into the conversation and survives restart
    Given a fresh launched fixture for "live-recording"
    When I record the real fixture audio from the empty transcript
